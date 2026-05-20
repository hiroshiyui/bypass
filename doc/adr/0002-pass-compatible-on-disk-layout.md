<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Pass-compatible on-disk store layout

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

A password manager's on-disk format is its most load-bearing interface:
once users have a store, they want to be able to read it tomorrow with a
different client, on a different machine, possibly years later. We need to
decide whether `bypass` invents its own format or reuses an existing one.

[`pass`](https://www.passwordstore.org/) is the de-facto Unix
password-manager format. Its layout is:

```
~/.password-store/
├── .gpg-id                 # recipient key id(s) for this subtree
├── email/
│   ├── .gpg-id             # optional per-subtree override
│   ├── work.gpg            # OpenPGP-encrypted entry
│   └── personal.gpg
└── bank/
    └── visa.gpg
```

Entry contents: first line is the password, optional `key: value` lines
follow. `.gpg-id` files closest to an entry on the way up to the root
determine its recipients.

## Considered Options

* **Mirror pass exactly.**
* **A new format**, JSON or a custom binary container, encrypted as a
  whole; cleaner schema; needs a migration tool.
* **A new directory layout** that still uses OpenPGP per-entry, but with
  different metadata files (e.g. a TOML manifest at the root).

## Decision Outcome

Chosen option: **mirror pass exactly**.

* Users with an existing `~/.password-store` can point `bypass` at it
  unchanged. The CLI is meant to be a drop-in replacement, not a fork.
* Existing tooling (pass extensions, mobile clients like Android Password
  Store, browser extensions like passff) keeps working against the same
  store on disk. This matters for incremental migration and for users who
  want to keep one of their existing clients alongside `bypass`.
* The format is dead simple and battle-tested. Reinventing it is a tax we
  would pay on day one and every day after.
* It naturally fits the platform-delegated crypto decision
  ([ADR-0001](0001-platform-delegated-crypto.md)): each `.gpg` blob is
  opaque to `bypass-core`, and `.gpg-id` walk-up resolution is implemented
  on top of the [`Storage`](../../crates/bypass-core/src/storage.rs) trait.

### Consequences

* Good: zero-friction migration from `pass`; users keep their git history.
* Good: `bypass-core` never needs to define a schema — entries are
  newline-delimited text, recipient lists are newline-delimited key ids.
* Bad: we inherit pass's limitations — no per-entry metadata beyond the
  ad-hoc `key: value` convention, no atomic multi-entry transactions.
  Acceptable; extensions can add structured metadata inside an entry.
* Bad: the format ties us to OpenPGP forever (or until we ship a documented
  migration path). Acceptable given ADR-0001.

### Confirmation

The [`Storage`](../../crates/bypass-core/src/storage.rs) trait is
deliberately format-agnostic; the `.gpg` suffix and `.gpg-id` walk-up are
conventions enforced by the (future) `store::Store` orchestrator. Any PR
that changes the on-disk layout must update this ADR (or supersede it) and
ship a migration tool.
