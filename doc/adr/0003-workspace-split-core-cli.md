<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Workspace split: `bypass-core` library + per-frontend crates

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass` targets three frontends (Linux CLI, Android, browser extensions
via a native-messaging host) that should share business logic but
absolutely cannot share I/O code: each has its own crypto provider
(ADR-0001), its own filesystem story, and its own clipboard/UI story.

We need to decide how the code is organised so that:

1. Business logic (entry parsing, password generation, OTP, `.gpg-id` walk,
   path safety) is written once.
2. Frontend-specific code (`gpg` subprocess, `git2`, `arboard`, Android
   AIDL bindings, …) does not leak into the shared crate, where it would
   either fail to build on the other targets (`git2` on Android NDK needs
   special work, `arboard` won't link in WebAssembly, …) or pull in
   unwanted dependencies for everyone.

## Considered Options

* **Single crate with feature flags** (`--features cli`, `--features
  android`). One `Cargo.toml`, conditional `cfg`-gated modules.
* **Cargo workspace** with a portable `bypass-core` library and one
  binary/cdylib crate per frontend.
* **Separate repositories**, one per frontend, vendoring or git-submoduling
  the core.

## Decision Outcome

Chosen option: **Cargo workspace with a portable `bypass-core` plus
frontend crates** (`bypass-cli` now, `bypass-ffi` for Android and an
`extension/` TS tree later).

* Frontends pull in `bypass-core` as a path dependency; their own deps
  (`git2`, `arboard`, `clap`, …) stay in their own `Cargo.toml`.
* `bypass-core` MUST NOT depend on `git2`, `arboard`, or any subprocess
  crate. If a feature requires those, the trait it implements belongs in
  core but the implementation belongs in the frontend.
* Feature flags would have worked but encourage cross-cutting `cfg`
  spaghetti and make it too easy to accidentally pull a platform crate into
  the "portable" build. A hard crate boundary is enforced by `cargo`
  itself.
* Multiple repos would fragment issue tracking, CI, and review for what is
  fundamentally one product.

### Consequences

* Good: cross-platform discipline is enforced by the compiler — you cannot
  `use git2;` from `bypass-core`.
* Good: changing CLI dependencies (e.g. bumping `arboard`) does not affect
  the Android build matrix.
* Good: each frontend can have its own MSRV / target-triple constraints
  without holding the others back.
* Bad: a single conceptual change (say, adding a new `Storage` method)
  touches two crates and needs both to compile. Acceptable; the workspace
  builds them together.
* Bad: integration tests that exercise core *through* a frontend live in
  the frontend crate, not in core. Workable.

### Confirmation

The crate boundary is the enforcement mechanism. Reviewers must reject any
addition of a non-portable dependency to `crates/bypass-core/Cargo.toml`.
The roadmap and `CLAUDE.md` both restate this rule; this ADR is the canonical
source.
