// SPDX-License-Identifier: GPL-3.0-or-later

// Sync code lands in stages: 5.2.a (this commit) ships pairing in
// isolation; 5.2.b wires the CLI dispatch. Dead-code suppression keeps
// `cargo clippy -D warnings` clean until then.
#![allow(dead_code)]

//! P2P sync surface (Phase 5.2).
//!
//! Sub-modules:
//!
//! - [`identity`]: the per-device libp2p identity key
//!   ([ADR-0015](../../../doc/adr/0015-device-identity-key.md)).
//! - [`peers`]: the pinned-peer table at
//!   `$XDG_CONFIG_HOME/bypass/peers.toml`
//!   ([ADR-0012](../../../doc/adr/0012-pake-spake2.md)).
//! - [`transport`]: the request-response `Transport` trait + the
//!   `InProcessTransport` test fake
//!   ([ADR-0013](../../../doc/adr/0013-sync-transport-trait.md)).
//! - [`pairing`]: PAKE-from-PIN handshake
//!   ([ADR-0012](../../../doc/adr/0012-pake-spake2.md)).
//!
//! Sub-milestone 5.2.b will add a real `Libp2pTransport` and the
//! git-pack-over-libp2p sync core; 5.2.c the daemon. Phase 5.2.a (this
//! commit) ships pairing in isolation: pairing logic is fully
//! exercised over `InProcessTransport` in unit tests, with no real
//! networking yet.

pub mod daemon;
pub mod identity;
pub mod libp2p_transport;
pub mod merge_driver;
pub mod pairing;
pub mod peers;
pub mod ratelimit;
pub mod socket;
pub mod syncing;
pub mod transport;
pub mod watcher;
pub mod wire;
