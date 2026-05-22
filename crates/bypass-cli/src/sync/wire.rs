// SPDX-License-Identifier: GPL-3.0-or-later

//! Sync wire format. One request type carries the entire protocol:
//! `WantPackFrom { local_head, peer_head_seen }`. The responder replies
//! with `Pack { bytes }` (the git pack of `local_head` minus the
//! commits reachable from `peer_head_seen`) or `Err { reason }`.
//!
//! First-sync uses the same shape with `peer_head_seen = None`: the
//! responder packs everything reachable from its HEAD.
//!
//! Pack bytes are inline. A 50 MB cap (ADR-0016, landing in 5.2.b.iii)
//! lives at the protocol layer above; this module only describes the
//! wire shape.

use serde::{Deserialize, Serialize};

/// Magic prefix mixed into every sync frame so a stray pairing message
/// (or arbitrary garbage) is rejected before deserialisation.
pub const WIRE_MAGIC: [u8; 8] = *b"BYPS-SYN";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireMsg {
    pub magic: [u8; 8],
    pub body: WireBody,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WireBody {
    /// Initiator → responder: "send me everything you have that I
    /// don't, given my current HEAD and the last HEAD I saw from you."
    ///
    /// Both fields are git SHA-1s as hex strings; `None` means "no
    /// such commit known locally" (first sync, or never seen this
    /// peer's history).
    WantPackFrom {
        local_head: Option<String>,
        peer_head_seen: Option<String>,
    },
    /// Responder → initiator: the packed objects + the peer's HEAD at
    /// the moment the pack was built. Empty `bytes` is legal and means
    /// "I have nothing new for you" (no commits beyond `peer_head_seen`
    /// reachable from my HEAD).
    Pack {
        peer_head: Option<String>,
        bytes: Vec<u8>,
    },
    /// Responder → initiator: refusal. `reason` is human-readable and
    /// goes straight to the initiator's stderr.
    Err { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("sync wire format: {0}")]
    Decode(String),

    #[error("sync wire format: wrong protocol magic")]
    BadMagic,
}

pub fn encode(msg: &WireMsg) -> Vec<u8> {
    serde_json::to_vec(msg).expect("WireMsg is always serialisable")
}

pub fn decode(bytes: &[u8]) -> Result<WireMsg, WireError> {
    let msg: WireMsg =
        serde_json::from_slice(bytes).map_err(|e| WireError::Decode(e.to_string()))?;
    if msg.magic != WIRE_MAGIC {
        return Err(WireError::BadMagic);
    }
    Ok(msg)
}

/// Build a `WantPackFrom` request frame.
pub fn want_pack_from(local_head: Option<String>, peer_head_seen: Option<String>) -> WireMsg {
    WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::WantPackFrom {
            local_head,
            peer_head_seen,
        },
    }
}

/// Build a `Pack` reply frame.
pub fn pack(peer_head: Option<String>, bytes: Vec<u8>) -> WireMsg {
    WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::Pack { peer_head, bytes },
    }
}

/// Build an `Err` reply frame.
pub fn err(reason: impl Into<String>) -> WireMsg {
    WireMsg {
        magic: WIRE_MAGIC,
        body: WireBody::Err {
            reason: reason.into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_want_pack_from() {
        let msg = want_pack_from(Some("abc123".into()), None);
        let bytes = encode(&msg);
        let decoded = decode(&bytes).unwrap();
        match decoded.body {
            WireBody::WantPackFrom {
                local_head,
                peer_head_seen,
            } => {
                assert_eq!(local_head.as_deref(), Some("abc123"));
                assert!(peer_head_seen.is_none());
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn roundtrip_pack() {
        let msg = pack(Some("deadbeef".into()), vec![1, 2, 3, 4]);
        let bytes = encode(&msg);
        let decoded = decode(&bytes).unwrap();
        match decoded.body {
            WireBody::Pack { peer_head, bytes } => {
                assert_eq!(peer_head.as_deref(), Some("deadbeef"));
                assert_eq!(bytes, vec![1, 2, 3, 4]);
            }
            other => panic!("unexpected body: {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_bad_magic() {
        let mut msg = pack(None, vec![]);
        msg.magic = *b"NOPE-NOP";
        let bytes = serde_json::to_vec(&msg).unwrap();
        assert!(matches!(decode(&bytes), Err(WireError::BadMagic)));
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(decode(b"not json"), Err(WireError::Decode(_))));
    }
}
