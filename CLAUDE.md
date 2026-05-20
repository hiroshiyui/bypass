# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`bypass` is a pass-compatible password manager in Rust, intended to ship as a Linux CLI, an Android app, and Firefox/Chrome browser extensions. Business logic lives in a platform-agnostic `bypass-core` library; each frontend brings its own concrete implementations of the core's I/O traits.

**`doc/ROADMAP.md` is the source of truth** for design decisions, planned crate layout, and phased work. Read it before making structural choices. Update its checkboxes as work lands.

## Workspace layout

```
crates/
├── bypass-core/   # platform-agnostic library: traits + business logic
└── bypass-cli/    # Linux binary `bypass` (clap, gpg subprocess, git2, arboard)
```

Reserved for later phases (not present yet): `crates/bypass-ffi/` (UniFFI surface for Android), `extension/` (TypeScript WebExtension).

## Locked-in design decisions

These come from `doc/ROADMAP.md` and should not be revisited without the user:

- **Crypto is platform-delegated; core never speaks OpenPGP.** The CLI shells out to `gpg` (do *not* substitute `age` or pure-Rust OpenPGP in this path). Android will delegate to **OpenKeychain** via its OpenPGP AIDL service. Browser extensions delegate to the desktop binary running in native-messaging-host mode. Stores must remain pass-compatible — same on-disk layout (`<path>/<name>.gpg`, `.gpg-id` files walked up the tree to resolve recipients).
- **Versioning:** `git2` crate (not subprocess `git`), in `bypass-cli` only.
- **Sync:** git remotes first; LAN P2P is a stretch goal in Phase 5.2, not Phase 1.
- **`bypass-core` stays portable.** It must not depend on `git2`, `arboard`, or any subprocess crate. Anything platform-specific belongs in a frontend crate.
- **License:** GPL-3.0-or-later. Every new source file (`*.rs`, build scripts, future shell/Kotlin/TypeScript sources) must begin with an SPDX header:

  ```rust
  // SPDX-License-Identifier: GPL-3.0-or-later
  ```

  Use the comment syntax appropriate to the file type (`//`, `#`, `<!--`). Do not omit it on new files; do not add it to vendored third-party code.

## Commands

- Build: `cargo build --workspace`
- Run CLI: `cargo run -p bypass -- <subcommand> [args]`
- Test: `cargo test --workspace`
- Single test: `cargo test -p bypass-core <test_name>`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings`
- Format: `cargo fmt`

Edition is **2024** (see workspace `Cargo.toml`) — some older idioms from 2021-edition examples will not compile.

## Working on this codebase

- When adding a feature, find its milestone in `doc/ROADMAP.md` and tick the checkbox in the same change.
- Decide which crate owns the change. Pure logic (parsing, generation, store traversal, OTP) goes in `bypass-core` and is written against the traits. Anything touching `gpg`, the filesystem, `git2`, the clipboard, or stdin/stdout goes in `bypass-cli`.
- Build order in the roadmap is intentional: workspace seams → GPG path → core CRUD → git integration → generation/clipboard → structured entries/OTP/extensions → sync → browser extension → Android. Avoid jumping ahead (e.g., don't wire git auto-commit before `insert`/`edit` exist).
- Secrets in memory should be wrapped in `zeroize`'d buffers once that dependency is added; never log decrypted content or write it to non-tempfile paths.
- Tests that exercise GPG need a throwaway keyring under a temp `GNUPGHOME` — never touch the user's real keyring.
