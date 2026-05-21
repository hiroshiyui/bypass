<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sync testability: a `Transport` trait + in-process fake

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[Phase 5.2](../sync-p2p-evaluation.md) introduces a substantial new
code surface: PAKE pairing, git-pack-over-libp2p sync, an
auto-rebase policy, a daemon, and several failure paths. If that
surface is written directly against the libp2p `Swarm`, every
single test has to spin up a real libp2p stack — even pure-logic
tests of the pairing state machine or the rebase policy. The
cost: slow, flaky, async-everywhere, hard to TDD.

The pattern that's worked for the rest of `bypass` is to write
business logic against narrow traits and provide both a real and
a fake implementation
([ADR-0006](0006-trait-associated-error-types.md)). Sync should
do the same. This ADR records the seam.

A full design walk-through (alternatives, layers, decisions)
lives in the test-strategy section of
[`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md); this
ADR records only the conclusion.

## Considered Options

* **No abstraction.** Write the sync code directly against
  `libp2p::Swarm`. Simpler today; every test pays the libp2p
  spin-up cost forever.
* **Byte-stream / connection-oriented trait.** Model the transport
  as TCP-like streams. Most flexible but the in-process fake has
  to mock framing, which is its own bug surface.
* **High-level "sync session" trait.** Hide the wire entirely
  behind something like
  `sync_with_peer(peer_id) -> Result<()>`. Tests can't exercise
  individual message exchanges; the protocol layer becomes
  black-box-only.
* **Request-response trait.** Match libp2p's
  [`request-response`](https://docs.rs/libp2p-request-response/)
  protocol — symmetric send-bytes / receive-bytes operations with
  peer identity attached. Two implementations: real
  (`Libp2pTransport`) and in-process pair
  (`InProcessTransport`).

## Decision Outcome

Chosen option: **request-response `Transport` trait**, with two
implementations.

The trait shape (final form lives in 5.2.b code; this is the
contract):

```rust
pub trait Transport {
    type PeerId: Clone + Eq + Hash + Send + Sync + 'static;
    type Error: std::error::Error + Send + Sync + 'static;

    /// Send `req` to `peer`, await their response.
    async fn request(&self, peer: &Self::PeerId, req: Bytes)
        -> Result<Bytes, Self::Error>;

    /// Receive the next inbound request. The `Reply` handle is
    /// fulfilled by the caller to send the response.
    async fn next_request(&self)
        -> Result<(Self::PeerId, Bytes, Reply), Self::Error>;
}
```

Mirrors libp2p's `request-response` 1:1, so `Libp2pTransport` is a
thin adapter, and the protocol logic that lives above the trait
doesn't change shape regardless of which transport it's running
on.

### Where it lives

The trait, its implementations, and all sync code live in
`bypass-cli` for v1. A `bypass-sync` crate split (foreshadowed by
[ADR-0010](0010-p2p-transport-libp2p.md)) is deferred until the
code is sized to warrant it.

The trait deliberately does **not** live in `bypass-core`. The
sync layer is async (libp2p is async), pulls in `tokio`, and is
filesystem-and-network-shaped — all things
[ADR-0003](0003-workspace-split-core-cli.md) forbids in core.

### Test layers

Three layers, each catching a distinct class of bug:

| Layer | Backed by | What it covers | Runtime cost |
| --- | --- | --- | --- |
| Unit | `InProcessTransport` | All sync logic (pairing state machine, rebase policy, leak-audit-on-receive, conflict surfacing, error handling) | Milliseconds; deterministic |
| Loopback | Real libp2p, mDNS bypassed via direct dial | The libp2p adapter itself: codec, framing, identity, real timeouts | Seconds per test; small focused set |
| Daemon | `assert_cmd` spawning two `bypass sync daemon` processes | Process lifecycle, FS watcher, socket handling, `sync status` UX | Seconds; `#[ignore]` by default |

Loopback tests run in `cargo test --workspace` by default (small
focused set, no flake-prone mDNS). Daemon tests are
`#[ignore]`'d by default and runnable via
`cargo test -- --ignored` or a dedicated CI job — they trade
runtime for end-to-end confidence.

### Consequences

* Good: the sync state machine (the part with the most
  semi-distributed bugs) is exercised end-to-end without ever
  spawning a libp2p stack. PAKE failure handling, rebase policy,
  leak-audit-on-receive — all unit-testable.
* Good: keeps the escape-valve from
  [ADR-0010](0010-p2p-transport-libp2p.md) cheap. If we ever
  swap libp2p for CPace-based pairing, QUIC-only transport,
  or anything else, the swap is contained to one trait impl.
* Good: matches the established pattern from ADR-0006 (associated
  error types on narrow traits with multiple impls). Readers of
  the codebase will recognise the shape.
* Bad: a request-response trait can't express pub-sub. We don't
  need pub-sub at the 2–5 device fleet sizes Phase 5.2 targets,
  but if that ever changes the trait has to grow.
* Bad: `InProcessTransport` is *too* deterministic — it can mask
  bugs that only show up with real network latency or
  out-of-order delivery. Mitigation: the loopback layer catches
  ordering bugs; we keep the loopback tests genuinely exercising
  the real libp2p path.
* Bad: async-everywhere ripples into the sync module. The
  boundary stays at module level — sync code is async, the rest
  of `bypass-cli` (dispatch, store ops) stays sync. The dispatch
  layer creates a `tokio::runtime::Runtime` ad-hoc only for
  `bypass sync daemon` / `bypass sync pair`.

### Confirmation

* `bypass-core/Cargo.toml` MUST NOT depend on libp2p or tokio
  ([ADR-0003](0003-workspace-split-core-cli.md)).
* Every PR adding sync-layer code should ship at least one unit
  test running against `InProcessTransport`. Reviewers should
  reject sync code that only has loopback tests — that's a sign
  the trait isn't being used as designed.
* The loopback test set deliberately stays small (one pairing
  exchange, one full sync roundtrip). New scenarios go into the
  unit tier unless they specifically exercise the libp2p
  adapter.

### Related

* [ADR-0003](0003-workspace-split-core-cli.md) — why this trait
  and its implementations can't live in `bypass-core`.
* [ADR-0006](0006-trait-associated-error-types.md) — same
  associated-`Error` pattern we already use for
  `Crypto`/`Storage`/`VersionControl`.
* [ADR-0010](0010-p2p-transport-libp2p.md) — the libp2p side of
  this seam. The `Transport` trait's existence is what keeps
  ADR-0010's "we can swap libp2p later" escape valve cheap.
* [ADR-0011](0011-sync-semantics-hybrid.md) — the protocol that
  flows through the trait.
* [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) —
  full walk-through, including the rejected alternatives.
