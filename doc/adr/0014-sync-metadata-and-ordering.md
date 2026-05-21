<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sync metadata: git commit fields only, no sidecars, no wall-clock ordering

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0011](0011-sync-semantics-hybrid.md) settled the wire format
and conflict policy for LAN P2P sync (git pack files, hybrid
auto-rebase, manual fallback) but left two questions open that
the eval doc flagged:

- *Clock handling for any per-entry metadata we might add* — do
  we need timestamps and last-writer identifiers beyond what git
  already records?
- *Rebase tie-breaker for symmetric divergence* — when both peers
  diverged with disjoint commits, who rebases onto whom?

Both questions converge on the same architectural choice: how
much metadata does Phase 5.2 add to the on-disk store, and how
much of the sync machinery is allowed to consult wall-clock time
across devices?

The temptation is to add per-entry timestamps. The cost is
real: a new file class (which the leak-audit
[ADR-0009](0009-leak-check-before-push.md) has to allowlist), a
new on-disk format (which pass and other tooling have to ignore
gracefully), and worst of all, an attack surface where a
compromised peer can backdate or future-date entries to win
"latest wins" comparisons.

## Considered Options

**For metadata storage:**

* **Nowhere new — git commit metadata only.** Reuse `author_time`,
  `committer`, `author_email` that every commit already carries.
* **Sidecar `*.gpg.meta` files** per entry. Plaintext (leaks
  access-pattern metadata) or encrypted (doubles OpenPGP cost
  per edit; needs new entry on every sync).
* **Header lines inside the encrypted body.** Bundled with the
  password; requires decryption to read metadata; precludes
  `bypass sync status` UX without a pinentry prompt.

**For symmetric-divergence ordering:**

* **Peer-ID lexical order** — lower peer-ID rebases onto the
  higher. Deterministic, clock-free, adversary-resistant.
* **Wall-clock author_time** — newer commit wins. Simple but
  clock-skew sensitive; compromised peer can backdate.
* **Commit-OID hash order** — lower OID wins. Clock-free but
  semantically arbitrary; harder to explain than peer-ID.

**For commit signing (per-peer attribution under attack):**

* **Defer to a future ADR.** Phase 5.2's threat model trusts
  paired peers up to the leak-audit's limits; cryptographic
  attribution is a separate concern.
* **Mandatory now** — every commit signed by the device's libp2p
  identity key.
* **Optional `--sign` flag** — opt-in.

## Decision Outcome

**Metadata storage: nowhere new.** Phase 5.2 uses git's existing
commit fields and adds no on-disk metadata.

**Tie-breaker: peer-ID lexical order.** When both peers diverged
with disjoint commits, the one whose peer-ID compares
lexically lower rebases onto the higher one. Both sides compute
the same answer locally without negotiation. No wall clock
consulted.

**Per-device attribution: via `user.name` / `user.email` at
daemon startup.** Each device sets git's commit-identity config
on the store repo to something stable and distinguishable —
suggested form `bypass-<friendly-name>` and
`<peer-id>@bypass.local` — at the first `bypass sync daemon`
launch (or earlier, at the first `bypass sync pair`). Friendly
name comes from `peers.toml`'s `name` field, defaulting to the
device's `hostname` if unset. `bypass sync status` reads
`git log -1 -- <entry>.gpg` to attribute the most recent edit.

**Commit signing: deferred to a future ADR.** Cryptographic per-
commit attribution (libp2p identity key signs each git commit;
peer verifies on receive) is genuinely useful but is its own
protocol layer with its own threat model and its own surface to
get wrong. Phase 5.2 ships without it; impersonation by a
compromised paired peer is a known residual risk (mitigated only
by the leak-audit catching plaintext, not by anything stopping a
peer pushing a *valid* ciphertext under another peer's name).

**Wall-clock usage is local-only.** The pairing PIN's 5-minute
timeout ([ADR-0012](0012-pake-spake2.md)) is the only place
`bypass` consults wall time, and that timeout is enforced on the
same device that generated the PIN — cross-device skew never
enters.

### Consequences

* Good: no new file class. The leak-audit allowlist stays exactly
  as ADR-0009 set it; pass-compat (ADR-0002) is unaffected. Other
  pass-compatible tools reading the same store see only `.gpg` and
  `.gpg-id` files plus the `.gitattributes` invariant.
* Good: ordering is clock-free and adversary-resistant in the
  ways an opaque-blob threat model cares about. A compromised peer
  cannot win a rebase by lying about timestamps because timestamps
  aren't consulted.
* Good: `bypass sync status` has enough information for useful UX
  (most-recent commit per entry, author identity, wall-time on the
  writing device) via `git log` with zero new format.
* Bad: `bypass sync status`'s attribution can be impersonated by
  a compromised paired peer (set any `user.name`, push). Mitigated
  only loosely until the future signing ADR. The eval doc records
  this in open question 7 (peer revocation), question 13 (mobile
  considerations), and the to-be-written signing ADR.
* Bad: the "timestamp on each device is whatever its wall-clock
  said" means `bypass log` across devices can show non-monotonic
  timestamps when one device's clock is wrong. Cosmetic; doesn't
  affect correctness.
* Bad: peer-ID lexical tie-breaker is *not* user-friendly to
  inspect — there's no human-readable reason "device A won" beyond
  "its peer-ID happened to sort lower". Mitigation is the
  `bypass sync status` view explaining which peer's commit ended
  up at HEAD after an auto-resolve.

### Confirmation

* Sub-milestone 5.2.b code MUST NOT introduce `.meta` files,
  encrypt additional fields into entry bodies, or consult wall-
  clock time for cross-device ordering. Reviewers should treat
  any of these as red flags.
* Sub-milestone 5.2.c daemon startup must set `user.name` and
  `user.email` in the store's git config to the bypass-device
  identity if they're not already set; existing user-configured
  values are preserved (the daemon will not clobber a deliberate
  override).
* The rebase tie-breaker is implemented as a string comparison
  on libp2p peer-ID; both peers run the same code and arrive at
  the same decision. A unit test in 5.2.b's planning must assert
  symmetric agreement across two `InProcessTransport`-connected
  fakes.
* This ADR explicitly does not address commit signing.
  Reviewers MAY require a "this is not the signing ADR" comment
  on PRs that touch commit-creation paths, to remind future
  contributors that a future ADR will revisit attribution.

### Related

* [ADR-0011](0011-sync-semantics-hybrid.md) — the rebase policy
  this ADR's tie-breaker plugs into.
* [ADR-0012](0012-pake-spake2.md) — peer-IDs (libp2p) are pinned
  by pairing; the tie-breaker uses those same IDs.
* [ADR-0009](0009-leak-check-before-push.md) — the leak-audit
  allowlist stays as-is because this ADR adds no new file class.
* [ADR-0002](0002-pass-compatible-on-disk-layout.md) — no new
  on-disk format; pass compatibility preserved.
* [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) —
  full context, including the open questions this ADR closes
  (#4 metadata/clock handling, #12 rebase tie-breaker) and the
  one it explicitly defers to a future ADR (cryptographic
  commit signing).
