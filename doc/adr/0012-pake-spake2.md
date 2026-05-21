<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Pairing PAKE: SPAKE2 via the `spake2` crate

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0010](0010-p2p-transport-libp2p.md) fixed the *shape* of LAN
pairing: one device displays a short PIN; the user types it on the
second device; both sides derive a shared secret from the PIN that
authenticates the first Noise handshake; afterwards, peer IDs are
pinned and the PIN is forgotten. That ADR deliberately deferred
the cryptographic primitive choice to a follow-up — this one.

The primitive question matters because a 6-digit PIN has only
~20 bits of entropy. If an attacker can observe the pairing
messages and brute-force the PIN *offline*, "pairing-by-PIN" is
purely UX theatre — the attacker just runs through all 10⁶
candidates in milliseconds and impersonates whichever side they
prefer. A real PAKE makes each guess require an online interaction
with one of the legitimate parties, which (combined with daemon-
side rate limiting) makes brute-force infeasible.

## Considered Options

* **`spake2`** (Rust crate). Symmetric PAKE; both parties know the
  same password and derive the same key. Used in production by
  `magic-wormhole-rs` for essentially our pairing flow since
  ~2016. Pure Rust, no external bindings.
* **CPace** (Rust crates exist). CFRG's 2023 preferred PAKE.
  Newer construction, simpler proof, narrower Rust ecosystem
  presence.
* **OPAQUE / aPAKE** family. State-of-the-art for client-server
  auth where the server holds a password verifier — the *shape* is
  wrong for symmetric peer-to-peer pairing.
* **Hand-rolled HKDF-of-PIN.** Derive a key directly from the PIN
  via HKDF. Trivially implementable; trivially broken: an attacker
  observing the handshake can brute-force the PIN offline.

## Decision Outcome

Chosen option: **`spake2`** (SPAKE2).

Concrete parameters fixed by this ADR:

- **PIN format**: 6 decimal digits, ~20 bits of entropy. Each
  digit is drawn from the OS CSPRNG.
- **PIN lifecycle**: single-use. Burned on first connection
  attempt (success *or* failure) and additionally expires after
  5 minutes if unused. Matches `magic-wormhole`'s behaviour.
- **Pairing direction**: symmetric. Either device can be the
  "show" side (`bypass sync pair --show`) and either the "enter"
  side (`bypass sync pair --enter`). The PAKE itself is
  symmetric so role assignment is purely UX.
- **Pinned peer state**: stored in a single
  `$XDG_CONFIG_HOME/bypass/peers.toml` (defaulting to
  `~/.config/bypass/peers.toml`). One file, list of records, atomic
  to update. Each record captures the libp2p peer ID, the peer's
  Noise static public key, an operator-supplied friendly name,
  and `paired_at`.
- **Daemon rate-limit**: after 3 failed PIN attempts within a
  60-second window, the daemon refuses further pairing attempts
  from any peer for 60 seconds. With the online cost forced by
  SPAKE2, this puts brute-force on the order of years even before
  the PIN expires.

Reasoning for SPAKE2 over the alternatives:

- **`magic-wormhole` precedent.** A decade of production use of
  SPAKE2 for almost exactly our pairing scenario (short PIN,
  bootstrap a secure channel, symmetric trust). Borrowing that
  exposure is the most "load-bearing audit" available without
  paying for one.
- **Rust ecosystem maturity.** The `spake2` crate is stable, has
  a clear feature surface, and is used by `magic-wormhole-rs`
  itself. CPace crates exist but have much less production bake
  time.
- **Localised future migration.** The pairing layer is a single
  trait-shaped seam in `bypass-cli` (or future `bypass-sync`). If
  CPace's Rust ecosystem matures and the project wants to switch,
  superseding this ADR and swapping the impl is a tractable
  change — not a rewrite.

### Consequences

* Good: pairing security rests on a well-studied primitive with
  real-world deployment. The "what if SPAKE2 is broken tomorrow"
  scenario is exactly the scenario `magic-wormhole`'s users would
  also face, and we'd hear about it.
* Good: pure-Rust implementation. No C bindings; clean cross-
  compilation to Android and (potentially) wasm.
* Good: 6-digit PIN is human-typeable on every device including
  phones; the entropy floor + online-only brute-force property is
  sufficient at the LAN threat model.
* Bad: SPAKE2 is not the CFRG-preferred construction in 2026
  (CPace is). We are explicitly choosing maturity over standards
  alignment, with the escape valve named above.
* Bad: `spake2` releases are infrequent — we should treat the
  pinned version as a security-relevant dependency and audit on
  upgrade.
* Bad: a 6-digit PIN is not robust against shoulder-surfing
  during the brief display window. Mitigation is operational
  (don't pair in a crowded place), not cryptographic. Users who
  want stronger entropy can paste a longer string via
  `bypass sync pair --pin <value>` (future ergonomic; this ADR
  reserves the slot).

### Confirmation

* Implementation lands in sub-milestone 5.2.a (device pairing).
  Reviewers should reject any PAKE call that uses
  `from_url`-style "ignore validation" shortcuts.
* The 6-digit PIN, 5-minute timeout, single-use lifecycle, and
  rate-limit are non-negotiable behaviour and should appear in
  unit tests for the pairing module before that sub-milestone
  merges.
* `bypass-core` MUST NOT depend on `spake2`; the dependency lives
  in `bypass-cli` (or `bypass-sync` if/when split) per
  [ADR-0003](0003-workspace-split-core-cli.md).
* Pinned-peer file format is a public-facing contract for
  third-party recovery tools and migrations; changes to it after
  5.2.a should supersede this ADR.

### Related

* [ADR-0010](0010-p2p-transport-libp2p.md) — set the shape of
  PAKE-from-PIN pairing; this ADR fills in the cryptographic
  primitive.
* [ADR-0001](0001-platform-delegated-crypto.md) — sync trust is
  deliberately *not* derived from the OpenPGP recipient key.
  PAKE-from-PIN is the alternative.
* [ADR-0011](0011-sync-semantics-hybrid.md) — what flows over the
  channel that PAKE bootstraps.
* [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) —
  full design walk-through, including the alternatives
  considered.
