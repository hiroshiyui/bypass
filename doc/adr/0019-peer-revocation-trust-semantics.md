<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Peer revocation trust semantics: history is final

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass sync pair` (5.2.a/b.i) records a peer in `peers.toml`.
That record is the trust anchor — every subsequent
`WantPackFrom` and Noise handshake verifies the peer's
identity against the pinned key. Phase 5.2.c needs the inverse
UX: "I lost my phone, untrust it." The eval doc flagged this
as [open question #7](../sync-p2p-evaluation.md), noting two
distinct sub-questions:

1. **The mechanic**: what CLI command, what does it touch?
2. **The trust semantics**: when a peer is revoked,
   do their *prior* commits remain trusted?

The first is straightforward (`bypass sync peer rm <name>`
removes the pin). The second has real implications:
[ADR-0014](0014-sync-metadata-and-ordering.md) deliberately
does not sign commits, so a revoked peer's history is
indistinguishable from any other history. We need to decide
whether that's a feature or a bug.

## Considered Options

**The mechanic:**

* **`bypass sync peer rm <name>`.** Mirrors `git remote
  remove`; the friendly name from pairing is the natural
  handle. The daemon (if running) reloads `peers.toml` after
  the change so subsequent inbound `WantPackFrom` from the
  removed peer-id is refused as "not a paired peer".
* `bypass sync revoke <peer-id>`. Strictly more precise but
  unfriendly — users remember `phone`, not `12D3KooW…`.
* GUI / interactive selection. Out of scope at this layer.

**The trust semantics — three positions:**

* **History is final** (do nothing to prior commits). The
  revoked peer's earlier commits sit in `.git/objects` and
  remain part of the user's history. Revocation prevents
  *future* contamination but doesn't rewrite the past.
* Best-effort rewrite via `git filter-branch` /
  `git filter-repo`. Walk every commit authored by the
  revoked peer's `user.name` / `user.email` and drop / rewrite
  it. Aggressive; rewrites SHAs (breaking other paired
  peers); relies on `user.name` which a compromised peer
  could have spoofed during ADR-0014's git-attribution
  window anyway.
* Tombstone marker. Write a `revoked: <peer-id>` line into
  the store and have the daemon flag any historical commit
  matching it. Adds noise; no real action; users can produce
  the same view with `git log --author`.

**Whether to require confirmation:**

* **Require `--yes` for the trust-semantics warning.** Force
  the user to see what revocation does and does not cover
  before mutating `peers.toml`.
* Silent removal. Trivial UX; risks the user thinking
  revocation rewrites history.

## Decision Outcome

- **Mechanic:** `bypass sync peer rm <name>` removes the
  matching record from `peers.toml` (atomic write, same path
  the existing `Peers::save` plumbing uses). If `<name>` is
  not found, exit non-zero with "no such peer". If multiple
  records share the same name (shouldn't happen — pairing
  upserts by peer-id, not name, but defensively), refuse and
  ask the user to specify by peer-id (which we'll plumb as
  `--peer-id` if/when a real user hits this).

- **Trust semantics: history is final.** Removing a peer
  prevents future syncs from that identity; it does not
  rewrite the local git history. This is consistent with the
  position [ADR-0014](0014-sync-metadata-and-ordering.md)
  already took: `bypass` does not cryptographically attribute
  individual commits, so there is no integrity guarantee to
  preserve in the past. A compromised paired peer's
  contributions were trusted at the time they landed; that
  trust window does not retroactively become un-trust.

- **Confirmation:** `bypass sync peer rm <name>` requires
  `--yes`. Without it, we print the warning paragraph below
  and exit 2 ("would have done X; re-run with `--yes`").

- **Warning text** (printed at confirmation and again on
  success):
  ```
  Removed pinning for 'phone' (12D3KooW…xyz).
  Future syncs with this peer are refused.

  Note: prior commits authored by this peer remain in your
  git history. `bypass` does not sign commits per ADR-0014,
  so we cannot reliably distinguish them after the fact. If
  you need a clean history, re-clone from a trusted source
  or use `git filter-repo` (see man bypass-sync).
  ```

- **Daemon refresh:** when the daemon is running, the CLI
  command updates `peers.toml` on disk and the daemon
  re-reads it on the next inbound request (file mtime check,
  cheap). No socket round-trip needed — the file *is* the
  durable trust anchor; the daemon is a cache.

## Consequences

### Good

- Predictable UX: revocation does what it says (stop
  syncing) and is explicit about what it doesn't (rewrite
  history).
- No SHA churn, no rewriting other peers' git state.
- Honest about [ADR-0014](0014-sync-metadata-and-ordering.md)'s
  trust model: we never claimed cryptographic per-commit
  attribution, so revocation can't pretend to enforce it.
- The `--yes` gate plus the warning paragraph mean a user
  who *thinks* revocation rewrites history finds out at the
  confirmation, not after the fact.

### Bad

- A user who actually wanted history-rewrite gets pointed at
  `git filter-repo` and has to do it themselves. We could
  ship a `bypass sync peer purge <name> --yes` helper later
  if real users hit this often; we'd want a future ADR
  before adding history-mutation primitives.
- The warning text is the *only* signal that the trust model
  is what it is. Users who skip docs and pass `--yes` blind
  will miss it. Acceptable trade-off given how rare
  revocation is and how the alternative (silent removal)
  is worse.

## Confirmation

- `bypass sync peer rm <name>` without `--yes`: exits 2,
  prints the warning, leaves `peers.toml` unchanged.
- `bypass sync peer rm <name> --yes`: removes record,
  prints the warning, exits 0.
- Unknown name: exits non-zero with "no such peer".
- The daemon refuses subsequent `WantPackFrom` from the
  removed peer-id with `WireBody::Err { reason: "not a
  paired peer" }`. Covered by the daemon integration test
  in [`crates/bypass-cli/tests/sync_daemon.rs`](../../crates/bypass-cli/tests/sync_daemon.rs)
  (extends the happy-path test with a revoke-and-retry leg).

## Related ADRs

- [ADR-0012](0012-pake-spake2.md): defines pairing, the
  forward operation this ADR reverses.
- [ADR-0014](0014-sync-metadata-and-ordering.md): explains
  why we can't cryptographically distinguish a revoked
  peer's prior commits from any other commit.
- [ADR-0015](0015-device-identity-key.md): `bypass sync
  identity rotate` is the *other* trust-cleaving operation;
  rotate clears `peers.toml` entirely (every paired peer is
  effectively revoked simultaneously), this ADR scopes the
  per-peer surgical case.
