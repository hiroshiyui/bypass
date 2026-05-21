<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Hybrid sync semantics: git pack on the wire, auto-rebase, manual fallback

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0010](0010-p2p-transport-libp2p.md) settles *how* devices
talk to each other (libp2p). This ADR settles *what* they say —
the application-layer semantics layered on top of that transport.

The shape of "sync" matters because it determines whether a
background daemon can advance state autonomously or whether every
sync round needs human attention. The full design walk-through
lives in [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md);
this ADR records only the decision and the rationale future
maintainers will need.

## Considered Options

* **A. Git-over-libp2p.** Peers are git remotes; sync is
  `git fetch` + `git push` over a libp2p transport.
  ([ADR-0004](0004-git2-crate-not-subprocess.md) keeps git as the
  in-process versioning layer, so the libgit2 plumbing for this
  is already half there.) Conflict resolution is identical to
  `bypass sync` from Milestone 5.1: rebase, human resolves.
* **B. Per-entry replication, last-write-wins.** The wire format
  is `(path, ciphertext, timestamp)` triples, not git objects.
  Each peer broadcasts (or pushes) updates. Conflicts resolved
  automatically by timestamp; the local git repo records each
  accepted update for history.
* **C. Hybrid: git pack on the wire, opinionated auto-rebase.**
  Wire format is git packs (same as A); semantics: fast-forward
  silently, auto-rebase on divergence with a "take-theirs for
  conflicting opaque blobs" policy, surface unresolvable cases
  to `bypass sync status` for manual handling.

## Decision Outcome

Chosen option: **C. Hybrid.**

Concretely:

1. Peers exchange git pack files over libp2p's request-response
   protocol (transport per ADR-0010).
2. When the peer's HEAD is a fast-forward of local HEAD → apply
   it silently. This is the common case for a 2–5 device personal
   fleet.
3. When histories have diverged on disjoint entries → run
   `git rebase --onto <peer>` locally. Resolution is automatic
   because the rebase doesn't touch the same files on both sides.
4. When the same `.gpg` blob is touched on both sides → use a
   recorded rebase strategy whose default is **take theirs**.
   Rationale: at the LAN sync rate we're optimising for, a peer
   reporting a different version of the same entry is far more
   likely to be the newer one (the user just edited it on the
   other device) than the result of two devices editing
   independently within seconds. The remaining true-conflict case
   is recoverable via point 5.
5. Genuine conflicts that the policy can't resolve, plus all
   blobs that fail the leak audit
   ([ADR-0009](0009-leak-check-before-push.md)), are queued under
   a `bypass sync status` view. The user runs `bypass edit
   <entry>` to hand-merge; the next exchange round clears them.

### Why not A

Git-over-libp2p as-is would make every divergence a manual
conflict. At the LAN sync rate the daemon targets (re-sync on
every local change), that's a constant stream of papercuts.
Encrypted `.gpg` files are opaque to git's auto-merge, so even
trivial divergences need human attention.

### Why not B

Per-entry replication abandons git as the wire format, fragmenting
the system into two history models — git locally, something else
on the wire — and forces us to invent a metadata channel for
timestamps/peer IDs that the encrypted blob can't carry. Clock
skew silently loses edits. Re-introducing git semantics on top
("apply received entries as commits") gets you back to A but with
extra translation steps.

Hybrid keeps git as both substrate and wire format, and pushes
the only application-specific logic (the "take theirs" default
on opaque blob conflicts) into a small rebase strategy that's
easy to reason about and easy to disable per-peer if it ever
proves wrong.

### Consequences

* Good: the common case (two devices, edits don't overlap) is
  fully automatic — daemons can do their job without prompting.
* Good: the wire format is git packs, which means a future user
  can always recover by adding the other device as a git remote
  manually, falling back to `bypass sync` (Phase 5.1) without any
  P2P involvement. No format lock-in.
* Good: ADR-0009's leak-check applies symmetrically — every
  incoming pack's tree is audited before acceptance. A
  misbehaving peer can't push plaintext into us.
* Bad: "take theirs" is a real trust statement. A malicious peer
  could push a stale or attacker-chosen ciphertext for an entry
  and we'd accept it. Mitigations: incoming blobs are still
  encrypted to the user's recipient set (a non-recipient
  attacker can't decrypt to read or forge); the local git log
  preserves what *was* there, so a user noticing a wrong value
  can recover via `bypass log <entry>` + `bypass git checkout`.
* Bad: detecting "the same blob was touched on both sides" needs
  a per-rebase check, which we'll implement as a custom merge
  driver via `.gitattributes`. That driver lives in `bypass-cli`
  and is registered by `bypass init` so existing stores get it
  on the next sync.
* Bad: a true two-sided concurrent edit (both devices edit the
  same entry between syncs) silently picks one. Mitigation:
  `bypass sync status` warns when a conflict was auto-resolved
  in the last N seconds; user can inspect via `bypass log` and
  manually re-apply the lost edit.

### Confirmation

* The hybrid policy will be implemented as a custom git merge
  driver registered by `bypass init` (and lazily installed by
  `bypass sync` on stores that pre-date this ADR). The driver
  source lives in `bypass-cli` and is invoked by git via
  `.gitattributes`.
* Test strategy: a two-peer in-process libp2p harness exercises
  fast-forward, disjoint-divergence, same-blob-divergence (auto-
  resolved), and the manual-fallback queue. Sub-milestone 5.2.b
  will land that harness alongside the wire protocol code.
* The leak-audit symmetry is non-negotiable; sub-milestone 5.2.b
  reviewers should reject any code path that accepts incoming
  blobs without running `audit::check_files`.

### Open questions deferred to sub-milestones

1. Exact representation of the "auto-resolved conflict, you might
   want to review" log entry for `bypass sync status` — sub-
   milestone 5.2.c.
2. Whether the merge driver should signature-verify peer-provided
   blobs in addition to running the leak audit. Currently the
   user's GPG recipient key gates *reading*, not *trusting* a
   write; a future ADR may tighten this.
3. Per-entry vector-clock metadata as a future extension if the
   "take theirs" default proves wrong in practice.

### Related

* [ADR-0001](0001-platform-delegated-crypto.md) — sync layer
  never decrypts.
* [ADR-0002](0002-pass-compatible-on-disk-layout.md) — synced
  trees must remain pass-compatible.
* [ADR-0004](0004-git2-crate-not-subprocess.md) — git stays the
  versioning substrate; libp2p just moves packs between repos.
* [ADR-0009](0009-leak-check-before-push.md) — incoming packs
  audited symmetrically.
* [ADR-0010](0010-p2p-transport-libp2p.md) — what carries the
  packs.
