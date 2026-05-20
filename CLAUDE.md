# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`bypass` is a CLI password manager in Rust, modeled after [pass](https://www.passwordstore.org/). The project is in its initial scaffolding stage — `src/main.rs` is still the `cargo new` placeholder.

**`doc/ROADMAP.md` is the source of truth** for design decisions, planned crate layout, and phased work. Read it before making structural choices. Update its checkboxes as work lands.

## Locked-in design decisions

These come from `doc/ROADMAP.md` and should not be revisited without the user:

- **Crypto:** shell out to `gpg` (do *not* substitute `age` or pure-Rust OpenPGP). Stores must remain pass-compatible — same on-disk layout (`<path>/<name>.gpg`, `.gpg-id` files walked up the tree to resolve recipients).
- **Versioning:** `git2` crate (not subprocess `git`).
- **Sync:** git remotes first; LAN P2P is a stretch goal in Phase 5.2, not Phase 1.

## Commands

- Build: `cargo build`
- Run: `cargo run -- <subcommand> [args]`
- Test: `cargo test`
- Single test: `cargo test <test_name>`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt`

Edition is **2024** (see `Cargo.toml`) — some older idioms from 2021-edition examples will not compile.

## Working on this codebase

- When adding a feature, find its milestone in `doc/ROADMAP.md` and tick the checkbox in the same change.
- Build order in the roadmap is intentional: GPG path → core CRUD → git integration → generation/clipboard → structured entries/OTP/extensions → sync. Avoid jumping ahead (e.g., don't wire git auto-commit before `insert`/`edit` exist).
- Secrets in memory should be wrapped in `zeroize`'d buffers once that dependency is added; never log decrypted content or write it to non-tempfile paths.
- Tests that exercise GPG need a throwaway keyring under a temp `GNUPGHOME` — never touch the user's real keyring.
