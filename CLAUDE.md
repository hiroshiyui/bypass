# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`bypass` is a pass-compatible password manager in Rust, intended to ship as a Linux CLI, an Android app, and Firefox/Chrome browser extensions. Business logic lives in a platform-agnostic `bypass-core` library; each frontend brings its own concrete implementations of the core's I/O traits.

**`doc/ROADMAP.md` is the source of truth** for design decisions, planned crate layout, and phased work. Read it before making structural choices. Update its checkboxes as work lands.

**`doc/adr/`** holds the project's [Architecture Decision Records](doc/adr/README.md) — the durable rationale behind each load-bearing design decision. Read the relevant ADRs before changing anything they cover; if a change reverses or extends a recorded decision, write a new ADR rather than editing the old one.

## Workspace layout

```
crates/
├── bypass-core/   # platform-agnostic library: traits + business logic
└── bypass-cli/    # Linux binary `bypass`
    ├── src/
    │   ├── main.rs           # dispatch
    │   ├── cli.rs            # clap derive command enum
    │   ├── crypto_gpg.rs     # impl Crypto via `gpg` subprocess
    │   ├── storage_fs.rs     # impl Storage on local FS (shred-on-remove)
    │   ├── vcs_git2.rs       # impl VersionControl via libgit2
    │   ├── audit.rs          # leak-check audit (ADR-0009)
    │   ├── clipboard.rs      # arboard + auto-clear daemon
    │   ├── doctor.rs         # `bypass doctor` env probe
    │   ├── edit.rs           # `bypass edit` tempfile + $EDITOR
    │   ├── extensions.rs     # pass-style extension dispatch
    │   ├── tree.rs           # ASCII tree renderer for `bypass ls`
    │   └── sync/             # Phase 5.2 LAN P2P sync surface
    │       ├── identity.rs           # Ed25519 identity key (ADR-0015)
    │       ├── peers.rs              # peers.toml pinned-peer table (ADR-0012)
    │       ├── transport.rs          # Transport trait + InProcessTransport (ADR-0013)
    │       ├── libp2p_transport.rs   # real libp2p Transport (ADR-0010)
    │       ├── pairing.rs            # SPAKE2 PAKE-from-PIN (ADR-0012)
    │       ├── wire.rs               # WantPackFrom / Pack / Err frames
    │       ├── syncing.rs            # pack build/ingest + reconcile (ADR-0011/0014/0016)
    │       ├── merge_driver.rs       # `bypass-take-theirs` (ADR-0011)
    │       ├── ratelimit.rs          # per-peer attempt window (ADR-0016)
    │       ├── socket.rs             # daemon status socket (ADR-0017/0018)
    │       ├── watcher.rs            # notify-based fs watcher
    │       └── daemon.rs             # `bypass sync daemon` main loop
    └── tests/                # integration tests against the real binary
        ├── common/mod.rs     # throwaway GNUPGHOME + TestEnv helper
        ├── end_to_end.rs     # default-suite CRUD / sync / pairing-CLI tests
        ├── sync_loopback.rs  # #[ignore]: two-process pair via libp2p
        └── sync_daemon.rs    # #[ignore]: daemon lifecycle + mDNS round-trip
```

Reserved for later phases (not present yet): `crates/bypass-ffi/` (UniFFI surface for Android), `extension/` (TypeScript WebExtension).

## Locked-in design decisions

These come from `doc/ROADMAP.md` and should not be revisited without the user:

- **Crypto is platform-delegated; core never speaks OpenPGP.** The CLI shells out to `gpg` (do *not* substitute `age` or pure-Rust OpenPGP in this path). Android will delegate to **OpenKeychain** via its OpenPGP AIDL service. Browser extensions delegate to the desktop binary running in native-messaging-host mode. Stores must remain pass-compatible — same on-disk layout (`<path>/<name>.gpg`, `.gpg-id` files walked up the tree to resolve recipients).
- **Versioning:** internal auto-commits use the `git2` crate (libgit2) in `bypass-cli` only — see [`vcs_git2.rs`](crates/bypass-cli/src/vcs_git2.rs). The user-facing `bypass git <args…>` subcommand spawns the system `git` binary; libgit2 is for typed in-process operations the orchestrator drives, subprocess git is for arbitrary user porcelain (push, pull, rebase, log formatting). Keep this boundary intact: do not extend the `bypass git` passthrough with parsing-via-libgit2, and do not re-implement user-facing porcelain on top of `git2`.
- **Sync:** git remotes are the default ([ADR-0011](doc/adr/0011-sync-semantics-hybrid.md)); LAN P2P shipped in Phase 5.2 via libp2p ([ADR-0010](doc/adr/0010-p2p-transport-libp2p.md)), with SPAKE2 pairing ([ADR-0012](doc/adr/0012-pake-spake2.md)), per-peer pinning in `peers.toml`, a custom merge driver for opaque `.gpg` conflicts, peer-ID lexical tie-breaker for symmetric divergence ([ADR-0014](doc/adr/0014-sync-metadata-and-ordering.md)), and a foreground `bypass sync daemon` ([ADR-0017](doc/adr/0017-daemon-socket-location.md), [ADR-0018](doc/adr/0018-daemon-status-protocol.md), [ADR-0019](doc/adr/0019-peer-revocation-trust-semantics.md)). Service-management glue (systemd user unit) is Phase 6 ([ADR-0020](doc/adr/0020-daemon-service-supervision.md); macOS / launchd dropped per [ADR-0028](doc/adr/0028-drop-macos-support.md)).
- **`bypass-core` stays portable.** It must not depend on `git2`, `arboard`, `libp2p`, `tokio`, `notify`, or any subprocess crate ([ADR-0003](doc/adr/0003-workspace-split-core-cli.md)). Anything platform-specific belongs in a frontend crate.
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
- Secrets in memory must be wrapped in [`bypass_core::crypto::SecretBytes`](crates/bypass-core/src/crypto.rs) (zeroize-on-drop, `Debug` hides contents). Never log decrypted content; never write it to non-tempfile paths. Tempfiles holding plaintext (`bypass edit`) must be wiped through `storage_fs::overwrite_then_unlink` so the same shred-style guarantee from [ADR-0008](doc/adr/0008-secure-delete-via-overwrite.md) applies.
- Tests that exercise GPG need a throwaway keyring under a temp `GNUPGHOME` — never touch the user's real keyring. The pattern is established in `crypto_gpg::tests` (unit) and reused via `crates/bypass-cli/tests/common/mod.rs` (`TestEnv` helper) for integration tests under `crates/bypass-cli/tests/`.
- The integration tests under `crates/bypass-cli/tests/` come in two tiers ([ADR-0013](doc/adr/0013-sync-transport-trait.md)). The default suite (`end_to_end.rs`) is fast and runs on every `cargo test`. The two-process loopback tests (`sync_loopback.rs`, `sync_daemon.rs`) spawn real `bypass` child processes against `127.0.0.1` and are `#[ignore]`-by-default; run them with `cargo test -p bypass --test sync_loopback -- --ignored` / `--test sync_daemon`. The mDNS-driven data-flow test self-skips with a stderr diagnostic when the host has no IPv4 multicast routes.
