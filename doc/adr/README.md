<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Architecture Decision Records

This directory holds the project's [Architecture Decision
Records](https://adr.github.io/) (ADRs) — the durable record of *why*
`bypass` is the way it is.

Process and conventions are defined in
[ADR-0000](0000-record-architecture-decisions.md). In short:

* One MADR-format file per decision: `NNNN-kebab-title.md`.
* Numbers are allocated in commit order and never reused.
* Once accepted, ADRs are not rewritten — they are superseded by a new
  ADR.

## Index

| #    | Title                                                                   | Status   |
| ---- | ----------------------------------------------------------------------- | -------- |
| 0000 | [Record architecture decisions](0000-record-architecture-decisions.md)  | Accepted |
| 0001 | [Platform-delegated OpenPGP crypto](0001-platform-delegated-crypto.md)  | Accepted |
| 0002 | [Pass-compatible on-disk store layout](0002-pass-compatible-on-disk-layout.md) | Accepted |
| 0003 | [Workspace split: core library + frontend crates](0003-workspace-split-core-cli.md) | Accepted |
| 0004 | [`git2` crate (libgit2) for versioning, not subprocess](0004-git2-crate-not-subprocess.md) | Accepted |
| 0005 | [License under GPL-3.0-or-later with SPDX headers](0005-gpl-license-with-spdx-headers.md) | Accepted |
| 0006 | [Associated `Error` types on core traits](0006-trait-associated-error-types.md) | Accepted |
| 0007 | [`RelPath` newtype with traversal-safety invariants](0007-relpath-traversal-safety.md) | Accepted |
| 0008 | [Secure-delete via overwrite in `StorageFs::remove`](0008-secure-delete-via-overwrite.md) | Accepted |
| 0009 | [Refuse to push files that don't look like OpenPGP ciphertext](0009-leak-check-before-push.md) | Accepted |
| 0010 | [Use libp2p (mDNS + Noise + request-response) for LAN P2P sync](0010-p2p-transport-libp2p.md) | Accepted |
| 0011 | [Hybrid sync semantics: git pack on the wire, auto-rebase, manual fallback](0011-sync-semantics-hybrid.md) | Accepted |
| 0012 | [Pairing PAKE: SPAKE2 via the `spake2` crate](0012-pake-spake2.md) | Accepted |
| 0013 | [Sync testability: a `Transport` trait + in-process fake](0013-sync-transport-trait.md) | Accepted |
| 0014 | [Sync metadata: git commit fields only, no sidecars, no wall-clock ordering](0014-sync-metadata-and-ordering.md) | Accepted |
| 0015 | [Device identity key: Ed25519 in `$XDG_CONFIG_HOME/bypass/identity.key`](0015-device-identity-key.md) | Accepted |
