<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Use libp2p (mDNS + Noise + request-response) for LAN P2P sync

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[Phase 5.2](../ROADMAP.md) ships LAN P2P sync so two or more of a
user's personal devices can stay in agreement without a shared
git remote. Before writing any of it we need to lock in *how*
devices find each other, *how* they prove identity, and *how* the
bytes move. That's the transport question; the format on the wire
is settled separately in [ADR-0011](0011-sync-semantics-hybrid.md).

A full design walk-through lives in
[`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md). This
ADR records only the conclusion and the reasons that matter for
future maintainers reading the code.

## Considered Options

* **libp2p** — the Parity-maintained Rust P2P stack. Provides
  mDNS discovery, Noise mutual authentication, TCP/QUIC
  transports, and a request-response protocol pattern out of the
  box. Used in production by IPFS, Polkadot, and others.
* **Custom protocol over plain TCP** with Noise (`snow`) for
  encryption, `mdns-sd` for discovery, hand-rolled framing.
  Minimal dependency surface but every wheel re-invented.
* **External daemons** (Syncthing, magic-wormhole) bolted on.
  Battle-tested in their own domains but neither models
  "ongoing replication of a small password store" cleanly, and
  the user would end up with overlapping sync layers.

## Decision Outcome

Chosen option: **libp2p**, using only the narrow slice we need:

- `libp2p-mdns` for LAN peer discovery.
- `libp2p-noise` for mutual authentication and forward-secret
  transport encryption.
- TCP for the actual transport (QUIC can come later if connection
  migration matters).
- `libp2p-request-response` for one-shot exchanges of git pack
  files (the wire format set by [ADR-0011](0011-sync-semantics-hybrid.md)).
  No gossipsub: at 2–5 personal devices the fan-out is small
  enough that point-to-point requests are simpler and the
  pub-sub overhead isn't earned.

Pairing uses **PAKE-from-PIN**: one device displays a one-shot
6-digit PIN, the other types it in, both sides derive a shared
secret used to authenticate the very first Noise handshake.
After that the peer IDs are pinned on each side and the PIN is
discarded. Concrete PAKE crate choice (`spake2` vs alternatives)
is deferred to its own follow-up ADR once the candidate has been
read end-to-end, but the *shape* — PAKE-authenticated bootstrap,
then per-peer pinning — is fixed by this ADR. We deliberately do
*not* couple sync trust to the OpenPGP recipient key (would force
the daemon to talk to gpg, violating ADR-0001's spirit and
inserting a pinentry into background sync).

Implementation lives in `bypass-cli` for v1. If the code grows
beyond a few hundred lines and another frontend wants to share
it, the natural split is a new `bypass-sync` crate; we defer that
decision until the code is sized.

### Consequences

* Good: discovery, encrypted-authenticated transport, retry
  semantics, NAT-friendliness, and peer identity are all provided
  by an audited library with a real user base, not reinvented.
* Good: libp2p's transport layer is pluggable, so a future
  Android or browser frontend can pick its own concrete transport
  (e.g. QUIC for Android) without changing the application
  protocol.
* Good: pairing via PAKE-from-PIN is well-understood (the same
  shape magic-wormhole uses) and lets us authenticate the first
  contact without a CA or central directory.
* Bad: ~50 transitive dependencies, ~30 s additional incremental
  build time, larger compiled binary. Acceptable for a CLI that
  the user installs once.
* Bad: libp2p minor versions break APIs more often than typical.
  Mitigation: pin to a specific minor, audit changelogs on
  upgrade, keep the surface area we use small.
* Bad: pulls in `tokio` (libp2p is async). The sync code is
  therefore async; the rest of `bypass-cli` stays sync. That
  boundary needs to be tidy — `tokio::runtime::Runtime` lives in
  the daemon module, never leaks into the dispatch layer.

### Confirmation

* The evaluation walk-through in
  [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) covers
  the threat model and the alternatives considered.
* No code yet; the next sub-milestone (5.2.a, pairing) will
  introduce the first concrete `libp2p` imports. PRs touching
  sync code should not add libp2p protocols beyond
  `{mdns, noise, request-response}` without superseding this
  ADR.
* `bypass-core/Cargo.toml` MUST NOT depend on `libp2p` (ADR-0003).

### Related

* [ADR-0001](0001-platform-delegated-crypto.md) — core never
  speaks OpenPGP; the sync layer never decrypts either.
* [ADR-0003](0003-workspace-split-core-cli.md) — the libp2p
  surface lives outside `bypass-core`.
* [ADR-0004](0004-git2-crate-not-subprocess.md) — git stays the
  local history substrate; libp2p is the *transport*, not the
  storage.
* [ADR-0009](0009-leak-check-before-push.md) — the same audit
  that gates `bypass sync` also gates any P2P transmission.
* [ADR-0011](0011-sync-semantics-hybrid.md) — what we send over
  libp2p (git pack files, hybrid auto-rebase policy).
