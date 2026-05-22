// SPDX-License-Identifier: GPL-3.0-or-later

//! PAKE-from-PIN device pairing.
//!
//! Implements the protocol sketched in
//! [ADR-0012](../../../../doc/adr/0012-pake-spake2.md):
//!
//! 1. One device displays a freshly-generated 6-digit PIN (the "show"
//!    side); the user types it on the other device (the "enter" side).
//! 2. Both sides run symmetric SPAKE2 with the PIN as the password and
//!    a shared protocol identifier. SPAKE2 finishes with a shared key
//!    `K` on both sides iff both ran with the same PIN.
//! 3. Both sides exchange a short key-confirmation tag derived from
//!    `K`; mismatch → wrong PIN, abort.
//! 4. Both sides exchange their (peer-id, identity-public-key) bundle.
//!    Because SPAKE2 (plus key confirmation) proves the channel is
//!    authenticated to a peer who knew the PIN, this exchange happens
//!    "in the clear" over the [`Transport`] in this sub-milestone.
//!    Sub-milestone 5.2.b will wrap the post-PAKE channel in Noise so
//!    the identity exchange is encrypted on the wire too — see ADR-0012's
//!    "PAKE-authenticated Noise handshake" note.
//! 5. Both sides record a [`PeerRecord`] in `peers.toml`.
//!
//! This module deliberately stops short of running an actual Noise
//! handshake on top of the SPAKE2-derived `K`. The InProcessTransport
//! tests run in a single process where MITM is impossible by
//! construction; 5.2.b will introduce libp2p's `noise` adapter on top.
//! The pairing **state machine** — PIN generation, SPAKE2 exchange,
//! key-confirmation, identity exchange, peer pinning — is fully
//! exercised here.

use std::time::Duration;

use anyhow::Result;
use libp2p_identity::{Keypair, PeerId};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};

use super::peers::PeerRecord;
use super::transport::Transport;

/// Single-protocol identifier mixed into SPAKE2 to domain-separate
/// `bypass` pairing from any other PAKE that might happen to share
/// the same PIN entropy.
const PROTOCOL_ID: &[u8] = b"bypass-pair-v1";

/// Length of a pairing PIN, in decimal digits.
pub const PIN_LEN: usize = 6;

/// Magic-wormhole-style PIN lifetime
/// ([ADR-0012](../../../../doc/adr/0012-pake-spake2.md)). Enforcement
/// is the caller's responsibility (the daemon in 5.2.c); this constant
/// is the canonical value sub-milestones should refer to.
pub const PIN_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// Wire-format message exchanged over the transport during pairing.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireMsg {
    /// Magic to reject non-bypass traffic on the wire early.
    magic: [u8; 8],
    body: WireBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
enum WireBody {
    /// Outgoing SPAKE2 message from the initiator. The recipient's
    /// reply (in the request-response shape of `Transport::request`)
    /// carries the responder's SPAKE2 message plus its key
    /// confirmation tag.
    Spake { spake_msg: Vec<u8> },
    /// Replied to the SPAKE message: the peer's SPAKE2 message and a
    /// key-confirmation tag.
    SpakeAck {
        spake_msg: Vec<u8>,
        confirmation: [u8; 32],
    },
    /// Second request: the initiator's identity bundle, plus its own
    /// key-confirmation tag (so the responder can be sure the
    /// initiator agrees on `K`).
    Identity {
        confirmation: [u8; 32],
        bundle: IdentityBundle,
    },
    /// Responder's reply to the identity exchange.
    IdentityAck { bundle: IdentityBundle },
}

const WIRE_MAGIC: [u8; 8] = *b"BYPS-PAR";

/// The pair-able bundle one side sends to the other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityBundle {
    /// User-chosen friendly name (e.g. "laptop", "phone").
    pub name: String,
    /// libp2p peer id, base58.
    pub peer_id: String,
    /// The peer's public identity key, hex-encoded protobuf.
    pub identity_pubkey_hex: String,
}

impl IdentityBundle {
    pub fn from_local(kp: &Keypair, name: impl Into<String>) -> Result<Self> {
        let pubkey_bytes = kp.public().encode_protobuf();
        Ok(Self {
            name: name.into(),
            peer_id: PeerId::from(kp.public()).to_base58(),
            identity_pubkey_hex: hex_encode(&pubkey_bytes),
        })
    }
}

/// Pairing-side failures.
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    #[error("invalid PIN: must be {PIN_LEN} digits, got {got:?}")]
    InvalidPin { got: String },

    #[error("SPAKE2 handshake failed: {0}")]
    Spake2(String),

    #[error("key-confirmation mismatch (most likely cause: wrong PIN entered)")]
    BadConfirmation,

    #[error("wire format error: {0}")]
    Wire(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("peer disconnected before pairing completed")]
    PeerDropped,
}

/// Generate a fresh 6-digit PIN from the OS CSPRNG.
pub fn generate_pin() -> String {
    let n: u32 = rand::rng().random_range(0..1_000_000);
    format!("{n:0width$}", width = PIN_LEN)
}

/// Validate a PIN's format. Empty / non-numeric / wrong length is an
/// `InvalidPin` error.
pub fn validate_pin(pin: &str) -> Result<(), PairingError> {
    if pin.len() != PIN_LEN || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(PairingError::InvalidPin {
            got: pin.to_owned(),
        });
    }
    Ok(())
}

/// Result of a successful pairing: the peer's bundle, the SPAKE2
/// shared key (kept for post-pairing tests; not persisted), and a
/// ready-to-insert [`PeerRecord`].
#[derive(Debug)]
pub struct PairedPeer {
    pub remote: IdentityBundle,
    pub record: PeerRecord,
}

/// "Show side": generate-or-take the PIN, await an inbound pairing
/// request from `peer` on `transport`, complete the handshake.
pub async fn run_show_side<T: Transport>(
    transport: &T,
    pin: &str,
    local: &Keypair,
    local_name: impl Into<String>,
) -> Result<PairedPeer, PairingError> {
    validate_pin(pin)?;
    let local_bundle = IdentityBundle::from_local(local, local_name.into())
        .map_err(|e| PairingError::Wire(e.to_string()))?;

    // First inbound message: the enter-side's SPAKE2 message.
    let (peer_id, first_bytes, first_reply) = transport
        .next_request()
        .await
        .map_err(|e| PairingError::Transport(e.to_string()))?;
    let first: WireMsg = decode_wire(&first_bytes)?;
    let peer_spake_msg = match first.body {
        WireBody::Spake { spake_msg } => spake_msg,
        other => {
            return Err(PairingError::Wire(format!("expected Spake, got {other:?}")));
        }
    };

    // Run SPAKE2 (we're "B" in asymmetric terms — the responder). We
    // use the symmetric variant because both sides know the same
    // password and we don't need role asymmetry.
    let (state, our_spake_msg) =
        Spake2::<Ed25519Group>::start_symmetric(&Password::new(pin), &Identity::new(PROTOCOL_ID));
    let key = state
        .finish(&peer_spake_msg)
        .map_err(|e| PairingError::Spake2(e.to_string()))?;
    let our_confirmation = confirmation_tag(&key, b"show-confirms");

    // Reply with our SPAKE2 message + our confirmation.
    let ack = WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::SpakeAck {
            spake_msg: our_spake_msg,
            confirmation: our_confirmation,
        },
    };
    first_reply.send(encode_wire(&ack));

    // Second inbound message: enter-side's identity + their
    // confirmation tag (proves they got the same key).
    let (_peer_id_2, second_bytes, second_reply) = transport
        .next_request()
        .await
        .map_err(|e| PairingError::Transport(e.to_string()))?;
    let second: WireMsg = decode_wire(&second_bytes)?;
    let (their_conf, remote_bundle) = match second.body {
        WireBody::Identity {
            confirmation,
            bundle,
        } => (confirmation, bundle),
        other => {
            return Err(PairingError::Wire(format!(
                "expected Identity, got {other:?}"
            )));
        }
    };
    let expected = confirmation_tag(&key, b"enter-confirms");
    if !constant_time_eq(&their_conf, &expected) {
        return Err(PairingError::BadConfirmation);
    }

    // Reply with our identity bundle.
    let ack = WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::IdentityAck {
            bundle: local_bundle,
        },
    };
    second_reply.send(encode_wire(&ack));

    let record = PeerRecord {
        name: remote_bundle.name.clone(),
        peer_id: remote_bundle.peer_id.clone(),
        noise_static_key: remote_bundle.identity_pubkey_hex.clone(),
        paired_at: now_rfc3339(),
    };

    let _ = peer_id; // suppress unused for the InProcessTransport case
    Ok(PairedPeer {
        remote: remote_bundle,
        record,
    })
}

/// "Enter side": user typed the PIN; initiate a pairing request to
/// `peer` on `transport`, complete the handshake.
pub async fn run_enter_side<T: Transport>(
    transport: &T,
    peer: &T::PeerId,
    pin: &str,
    local: &Keypair,
    local_name: impl Into<String>,
) -> Result<PairedPeer, PairingError> {
    validate_pin(pin)?;
    let local_bundle = IdentityBundle::from_local(local, local_name.into())
        .map_err(|e| PairingError::Wire(e.to_string()))?;

    // First round: send our SPAKE2 message; receive their SpakeAck.
    let (state, our_spake_msg) =
        Spake2::<Ed25519Group>::start_symmetric(&Password::new(pin), &Identity::new(PROTOCOL_ID));
    let first = WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::Spake {
            spake_msg: our_spake_msg,
        },
    };
    let first_reply_bytes = transport
        .request(peer, encode_wire(&first))
        .await
        .map_err(|e| PairingError::Transport(e.to_string()))?;
    let first_reply: WireMsg = decode_wire(&first_reply_bytes)?;
    let (peer_spake_msg, peer_confirmation) = match first_reply.body {
        WireBody::SpakeAck {
            spake_msg,
            confirmation,
        } => (spake_msg, confirmation),
        other => {
            return Err(PairingError::Wire(format!(
                "expected SpakeAck, got {other:?}"
            )));
        }
    };

    let key = state
        .finish(&peer_spake_msg)
        .map_err(|e| PairingError::Spake2(e.to_string()))?;
    let expected = confirmation_tag(&key, b"show-confirms");
    if !constant_time_eq(&peer_confirmation, &expected) {
        return Err(PairingError::BadConfirmation);
    }

    // Second round: send our identity + our confirmation; receive
    // their identity in response.
    let our_confirmation = confirmation_tag(&key, b"enter-confirms");
    let second = WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::Identity {
            confirmation: our_confirmation,
            bundle: local_bundle,
        },
    };
    let second_reply_bytes = transport
        .request(peer, encode_wire(&second))
        .await
        .map_err(|e| PairingError::Transport(e.to_string()))?;
    let second_reply: WireMsg = decode_wire(&second_reply_bytes)?;
    let remote_bundle = match second_reply.body {
        WireBody::IdentityAck { bundle } => bundle,
        other => {
            return Err(PairingError::Wire(format!(
                "expected IdentityAck, got {other:?}"
            )));
        }
    };

    let record = PeerRecord {
        name: remote_bundle.name.clone(),
        peer_id: remote_bundle.peer_id.clone(),
        noise_static_key: remote_bundle.identity_pubkey_hex.clone(),
        paired_at: now_rfc3339(),
    };

    Ok(PairedPeer {
        remote: remote_bundle,
        record,
    })
}

// ----- helpers ---------------------------------------------------------

fn confirmation_tag(key: &[u8], domain: &[u8]) -> [u8; 32] {
    // Domain-separated hash of the SPAKE2 output. Sufficient for
    // pairing-time key confirmation: an attacker who didn't run
    // SPAKE2 with the right password cannot produce a matching tag,
    // and the SPAKE2 output already incorporates the protocol-id
    // identity bound at handshake start.
    let mut hasher = Sha256::new();
    hasher.update(b"bypass-conf-v1");
    hasher.update(domain);
    hasher.update(key);
    let out = hasher.finalize();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&out);
    tag
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut acc: u8 = 0;
    for i in 0..32 {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

fn encode_wire(msg: &WireMsg) -> Vec<u8> {
    // Tiny custom framing rather than pulling a serde-compatible
    // binary codec: postcard + serde would also work but adds a
    // dependency for what is right now four message variants.
    serde_json::to_vec(msg).expect("WireMsg is always serialisable")
}

fn decode_wire(bytes: &[u8]) -> Result<WireMsg, PairingError> {
    let msg: WireMsg = serde_json::from_slice(bytes)
        .map_err(|e| PairingError::Wire(format!("decode pairing message: {e}")))?;
    if msg.magic != WIRE_MAGIC {
        return Err(PairingError::Wire(format!(
            "wrong protocol magic: {:?}",
            msg.magic
        )));
    }
    Ok(msg)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_rfc3339() -> String {
    // Wall-clock at the local device — see ADR-0014. We don't try to
    // be clever about format-rs vs chrono; "yyyy-mm-ddTHH:MM:SSZ" via
    // SystemTime + a tiny formatter is enough for the audit field.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch ≈ secs / 86400; cheap-and-cheerful with no
    // dep. Not a leap-year-correct calendar but sufficient for an
    // approximate audit timestamp.
    format_unix_ts(secs)
}

fn format_unix_ts(secs: u64) -> String {
    // Civil date from days-since-1970-01-01 using the Howard Hinnant
    // formula (public domain).
    let days = (secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    let rem = secs % 86_400;
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u8, u8) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::transport::InProcessTransport;

    fn fresh_keypair() -> Keypair {
        Keypair::generate_ed25519()
    }

    #[test]
    fn generate_pin_is_six_digits() {
        for _ in 0..100 {
            let pin = generate_pin();
            assert_eq!(pin.len(), PIN_LEN);
            assert!(pin.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn validate_pin_rejects_bad_inputs() {
        for bad in ["", "12345", "1234567", "12345a", "abcdef"] {
            assert!(
                matches!(validate_pin(bad), Err(PairingError::InvalidPin { .. })),
                "should reject {bad:?}"
            );
        }
        assert!(validate_pin("123456").is_ok());
    }

    #[test]
    fn confirmation_tag_is_deterministic_and_distinguished_by_domain() {
        let k = b"shared-secret-of-some-length-doesnt-matter";
        let a = confirmation_tag(k, b"domain-1");
        let b = confirmation_tag(k, b"domain-1");
        assert_eq!(a, b);
        let c = confirmation_tag(k, b"domain-2");
        assert_ne!(a, c);
    }

    #[tokio::test]
    async fn show_and_enter_complete_with_matching_pins() {
        let (a_t, b_t) = InProcessTransport::pair("show", "enter");
        let pin = "528491";
        let show_kp = fresh_keypair();
        let enter_kp = fresh_keypair();

        let show_pid = PeerId::from(show_kp.public()).to_base58();
        let enter_pid = PeerId::from(enter_kp.public()).to_base58();

        let show_fut = {
            let show_kp = show_kp.clone();
            async move { run_show_side(&a_t, pin, &show_kp, "show-device").await }
        };
        let enter_fut = {
            let enter_kp = enter_kp.clone();
            async move {
                run_enter_side(&b_t, &"show".to_string(), pin, &enter_kp, "enter-device").await
            }
        };
        let (show_result, enter_result) = tokio::join!(show_fut, enter_fut);
        let show = show_result.unwrap();
        let enter = enter_result.unwrap();

        // Each side learned the other's identity.
        assert_eq!(show.remote.peer_id, enter_pid);
        assert_eq!(enter.remote.peer_id, show_pid);
        assert_eq!(show.remote.name, "enter-device");
        assert_eq!(enter.remote.name, "show-device");

        // Both PeerRecords are well-formed for peers.toml insertion.
        assert_eq!(show.record.peer_id, enter_pid);
        assert!(!show.record.noise_static_key.is_empty());
    }

    #[tokio::test]
    async fn mismatched_pins_fail_with_bad_confirmation() {
        let (a_t, b_t) = InProcessTransport::pair("show", "enter");
        let show_kp = fresh_keypair();
        let enter_kp = fresh_keypair();

        let show_fut = {
            let show_kp = show_kp.clone();
            async move { run_show_side(&a_t, "111111", &show_kp, "show").await }
        };
        let enter_fut = {
            let enter_kp = enter_kp.clone();
            async move { run_enter_side(&b_t, &"show".to_string(), "222222", &enter_kp, "enter").await }
        };
        let (show_result, enter_result) = tokio::join!(show_fut, enter_fut);

        // Enter side checks confirmation first (in response to the
        // SpakeAck) so it surfaces the mismatch cleanly.
        let enter_err = enter_result.unwrap_err();
        assert!(
            matches!(enter_err, PairingError::BadConfirmation),
            "enter side should detect BadConfirmation, got {enter_err:?}"
        );
        // Show side was waiting for the next request when enter
        // aborted; it sees the dropped transport. The semantic that
        // matters at the UX layer: pairing failed on both sides.
        let show_err = show_result.unwrap_err();
        assert!(
            matches!(show_err, PairingError::Transport(_)),
            "show side should see a transport-level abort, got {show_err:?}"
        );
    }

    #[test]
    fn format_unix_ts_renders_known_epoch_dates() {
        // 2026-05-22T00:00:00Z = 1779408000 seconds since epoch
        // (20595 days × 86400 sec).
        assert_eq!(format_unix_ts(1_779_408_000), "2026-05-22T00:00:00Z");
        // 1970-01-01T00:00:00Z
        assert_eq!(format_unix_ts(0), "1970-01-01T00:00:00Z");
        // 2000-02-29T12:34:56Z — leap day on a leap century-year.
        // 30 years × 365 + 8 leap days (1972..2000 step 4 = 8) = 10957 days.
        // Jan(31) + Feb 1..28 (27 more) + 29 = day 60 of 2000 → +59 days into year.
        // Total: 10957 + 59 = 11016 days. + 45296 seconds (12:34:56).
        assert_eq!(
            format_unix_ts(11_016 * 86_400 + 12 * 3600 + 34 * 60 + 56),
            "2000-02-29T12:34:56Z"
        );
    }
}
