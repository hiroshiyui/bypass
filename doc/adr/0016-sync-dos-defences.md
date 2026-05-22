<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# DoS defences for incoming sync: pack-size cap + per-peer rate limit

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0010](0010-p2p-transport-libp2p.md) and
[ADR-0011](0011-sync-semantics-hybrid.md) settle the *protocol*
for LAN P2P sync: paired peers exchange git packs over a
libp2p request-response channel. The threat model in
[`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) names
two abuses that are *not* covered by the leak-audit
([ADR-0009](0009-leak-check-before-push.md)) and *are* possible
even from a peer that completed PAKE pairing
([ADR-0012](0012-pake-spake2.md)):

1. **Oversized packs.** A misbehaving peer (compromised, buggy,
   or hostile after pairing) sends a multi-gigabyte pack
   intended to fill the receiver's disk before the leak-audit
   even runs.
2. **Flood.** The same peer hammers the responder with valid
   `WantPackFrom` requests, draining CPU on pack generation or
   socket descriptors.

The eval doc flagged this as **open question #8** and called
for "refuse-by-default with explicit caps, parallel to ADR-0009".
This ADR commits to concrete numbers and the enforcement points.

## Considered Options

**Pack-size cap:**

* **Hard cap at 50 MB**, refuse anything larger on both send
  and receive paths. Simple, predictable, generous for a
  password store (entries are KB-scale and git history
  compresses well).
* Soft cap with disk-quota check. Inspect free space, compute a
  fraction of it as the cap, refuse above that. More adaptive;
  more code; behaviour varies across machines, making
  bug-reports harder.
* No cap, trust paired peers. Matches the "paired peers are
  in your trust circle" tone of ADR-0012, but breaks the
  threat-model commitment from the eval doc. A compromised
  peer is still a paired peer.
* Per-pack streaming with a running byte counter and refusal
  partway through. Saves disk space on the truncation path but
  requires libp2p to surface request bytes incrementally —
  `request-response` does not (it materialises the full request
  before handing it to the application). Out of reach without
  a different transport.

**Rate limit:**

* **3 attempts per 60-second window, per peer-id**, mirroring
  [ADR-0012](0012-pake-spake2.md)'s PAKE rate-limit shape so
  there is one rule for users to internalise.
* Token bucket. Smoother; more state per peer; no compelling
  reason given how rare legitimate sync attempts are.
* Global rate limit (not per-peer). Catches a botnet-style
  flood but rewards an attacker for spreading load across many
  paired peers; per-peer is strictly better given that we
  already pin peer identities.
* No rate limit, rely on libp2p connection caps. libp2p's
  default connection limits don't help against a peer that
  *legitimately* completed Noise; it's already in.

**Enforcement scope:**

* In the **`bypass-cli` sync layer** (in
  [`sync::syncing`](../../crates/bypass-cli/src/sync/syncing.rs)
  and a new
  [`sync::ratelimit`](../../crates/bypass-cli/src/sync/ratelimit.rs))
  rather than libp2p configuration. The cap is a protocol
  guarantee; libp2p just carries bytes.
* In libp2p `request-response`'s `Config::set_max_request_size`.
  Available but applies symmetrically to *all* requests on the
  protocol — pack bodies dominate, but pairing frames are also
  on the same path. Mixing concerns and keeping the cap in
  config rather than in code makes the policy harder to see.

## Decision Outcome

- **Pack-size cap:** **50 MB**, exposed as
  [`sync::syncing::MAX_PACK_BYTES`](../../crates/bypass-cli/src/sync/syncing.rs)
  (already stubbed in 5.2.b.ii). The cap is enforced
  symmetrically:
  - **Receive:** the initiator refuses to ingest a `Pack` whose
    `bytes.len() > MAX_PACK_BYTES`. Already implemented; this
    ADR retroactively justifies it.
  - **Send:** the responder refuses to *build* a pack that
    would exceed the cap. If `build_pack` produces more than
    `MAX_PACK_BYTES` it returns a `WireBody::Err` instead of a
    `Pack`, with a human-readable reason naming the cap.
  Users hitting the wall on a legitimate migration can split
  the sync into smaller windows by syncing intermediate commits
  via a regular git remote; that pressure-release is
  documented in the eval doc, not in user-facing UI.

- **Rate limit:** **3 sync attempts per 60-second window, per
  peer-id**, implemented in a new
  [`sync::ratelimit`](../../crates/bypass-cli/src/sync/ratelimit.rs)
  module as an in-memory
  `HashMap<PeerId, AttemptHistory>`. Each attempt records a
  `Instant`; on a new attempt the bucket prunes entries older
  than 60 s, then accepts if the bucket is below 3 and refuses
  otherwise. State is process-local: the one-shot `bypass
  sync` path gates outbound attempts to its paired peers; the
  daemon (Phase 5.2.c) holds the inbound bucket per peer and
  rejects WantPackFrom requests over the limit with a
  `WireBody::Err`.

- **Enforcement scope:** both decisions live in the sync layer
  of `bypass-cli`. libp2p config is left at defaults except
  where the cap demands it: `request-response` `Config` is
  given a `set_max_request_size(MAX_PACK_BYTES + 64 * 1024)`
  margin so a request that would have been packed at the cap
  isn't itself rejected for being one CBOR-framing byte over.

- **Bootstrap protocol** (eval-doc open question #11): the same
  `WantPackFrom` shape is the bootstrap. A device with no local
  HEAD sends `WantPackFrom { local_head: None, peer_head_seen:
  None }`; the responder packs everything reachable from its
  HEAD; the initiator fast-forwards onto it. There is *no*
  separate bootstrap message and the same pack-size cap applies
  — meaning the cap is also a per-clone size ceiling for now.
  Stores that grow past 50 MB will need to bootstrap via git
  remote first, then switch to peer sync for incremental
  updates.

## Consequences

### Good

- A buggy or hostile paired peer cannot trivially fill the
  receiver's disk or exhaust CPU. The leak-audit
  ([ADR-0009](0009-leak-check-before-push.md)) plus this ADR
  together cover the eval doc's threat model for incoming sync.
- The cap is a *single constant* the user can quote in a bug
  report, and the rate-limit shape matches
  [ADR-0012](0012-pake-spake2.md)'s PAKE rate-limit so the
  mental model is "three strikes per minute".
- Enforcement in the sync layer keeps libp2p configuration
  minimal; swapping the transport later (QUIC, mobile, etc.)
  doesn't lose the DoS posture.

### Bad

- **50 MB is a real ceiling.** A store migrated from another
  tool that accumulated large attachments (the eval-doc
  bootstrap path) can't be peer-synced in one round. The
  workaround is "use a git remote for the initial clone", which
  is documented in the eval doc but is a real UX trip-wire.
  We accept it for now and will revisit if any real user
  reports it.
- **The rate limit is process-local.** Two paired bypass
  instances on the same machine (unlikely but possible) each
  keep their own counters. The daemon model (Phase 5.2.c) is
  the natural place to centralise; the one-shot `bypass sync`
  inherits the limit only for its own lifetime, which is
  usually a few seconds.
- **Bootstrap = `WantPackFrom { None, None }`** means the
  responder always packs its full history on a first sync; a
  malicious initiator can lie about having no local HEAD to
  force the responder into the expensive path. This is bounded
  by the rate limit and pack-size cap, but is a small CPU
  asymmetry we accept rather than introduce a "do you already
  have my X" challenge phase.

## Confirmation

- `sync::syncing::MAX_PACK_BYTES = 50 * 1024 * 1024` is
  load-bearing for both the receive guard (`sync_with_peer`)
  and the send guard (`build_pack` / `serve_want_pack_from`).
  Unit tests in `crates/bypass-cli/src/sync/syncing.rs` exercise
  the refusal paths.
- `sync::ratelimit::AttemptLog` has unit tests in
  `crates/bypass-cli/src/sync/ratelimit.rs` covering the
  3-attempts-then-refuse and 60-s-window-expiry cases.
- Eval-doc open questions
  [#8](../sync-p2p-evaluation.md#open-questions-surfaced-by-the-first-design-pass)
  (DoS defences) and
  [#11](../sync-p2p-evaluation.md#open-questions-surfaced-by-the-first-design-pass)
  (bootstrap protocol) are closed by this ADR.

## Related ADRs

- [ADR-0009](0009-leak-check-before-push.md): mirrors this
  ADR's refuse-by-default stance on the outbound side
  (plaintext leak check before push).
- [ADR-0010](0010-p2p-transport-libp2p.md): defines the libp2p
  request-response surface this ADR caps.
- [ADR-0011](0011-sync-semantics-hybrid.md): defines the
  `WantPackFrom`/`Pack` protocol this ADR throttles.
- [ADR-0012](0012-pake-spake2.md): rate-limit shape mirrored.
