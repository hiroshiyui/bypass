# bypass — Roadmap

A pass-compatible password manager in Rust, targeting Linux CLI, Android, and Firefox/Chrome browser extensions from a shared core.

## Design decisions

- **Crypto is platform-delegated; the core library never speaks OpenPGP.** Each frontend brings its own OpenPGP provider:
  - **Linux CLI:** shell out to `gpg` (pass-compatible stores, easy migration).
  - **Android:** delegate to [OpenKeychain](https://www.openkeychain.org/) via its OpenPGP AIDL service (Kotlin-side, exposed back into Rust through a UniFFI callback interface).
  - **Browser extension:** delegate to the desktop `bypass` binary running in native-messaging-host mode.
- **Versioning:** git-backed via the `git2` crate, on platforms that have a filesystem.
- **Storage layout:** mirrors pass — `~/.password-store/<path>/<name>.gpg`, with `.gpg-id` files marking recipient keys per subtree.
- **Sync strategy:** start with git remotes (push/pull over SSH or shared LAN path); evaluate Syncthing-style P2P only after the core is stable.
- **Portability rule:** `bypass-core` must not depend on `git2`, `arboard`, or any subprocess crate. If a feature needs those, the trait it implements belongs in core but the implementation belongs in the frontend.

## Workspace layout

```
bypass/
├── Cargo.toml                       # workspace manifest
└── crates/
    ├── bypass-core/                 # platform-agnostic library
    │   └── src/
    │       ├── lib.rs
    │       ├── crypto.rs            # Crypto trait + SecretBytes
    │       ├── storage.rs           # Storage trait
    │       ├── vcs.rs               # VersionControl trait (optional impl)
    │       ├── path.rs              # RelPath newtype, traversal-safe
    │       ├── gpg_id.rs            # walk-up .gpg-id recipient resolution
    │       ├── store.rs             # Store<C,S,V> orchestrator
    │       ├── entry.rs             # multi-line entry parsing
    │       ├── generate.rs          # password generation
    │       └── otp.rs               # TOTP from otpauth:// URIs
    └── bypass-cli/                  # Linux binary `bypass`
        └── src/
            ├── main.rs              # clap dispatch
            ├── cli.rs               # clap derive command enum
            ├── crypto_gpg.rs        # impl Crypto via `gpg` subprocess
            ├── storage_fs.rs        # impl Storage on local FS
            ├── vcs_git2.rs          # impl VersionControl via git2
            ├── clipboard.rs         # arboard + auto-clear
            └── messaging_host.rs    # `bypass messaging-host` subcommand
```

Reserved for later phases: `crates/bypass-ffi/` (UniFFI surface for Android) and `extension/` (TypeScript WebExtension for Firefox & Chrome).

Core dependencies (planned): `rand`, `totp-rs`, `thiserror`, `zeroize` in `bypass-core`; `clap`, `git2`, `arboard`, `anyhow`, `serde_json`, `dirs` in `bypass-cli`.

---

## Phase 0.5 — Workspace split & trait seams

- [x] Convert root to a Cargo workspace
- [x] Create `bypass-core` crate with module skeletons
- [x] Move CLI binary into `bypass-cli` crate
- [x] Define `Crypto`, `Storage`, `VersionControl` traits in `bypass-core`
- [x] Define `RelPath` newtype with traversal-safety invariants
- [x] Define `SecretBytes` (zeroize-wrapped) and core error types

---

## Phase 1 — Foundations *(business logic in `bypass-core`, I/O in `bypass-cli`)*

### Milestone 1.1: Project skeleton — `bypass-cli`
- [x] Add core dependencies to both crate manifests
- [x] Set up `cli.rs` with clap derive command enum
- [x] Wire `main.rs` to dispatch subcommands
- [x] Add `anyhow`/`thiserror` error scaffolding
- [x] Confirm `.gitignore` covers `target/` and any test fixtures

### Milestone 1.2: GPG crypto path — `crypto_gpg` in `bypass-cli` against `Crypto` in `bypass-core`
- [x] `crypto_gpg.rs`: `encrypt(plaintext, recipients) -> Vec<u8>` via `gpg` subprocess
- [x] `crypto_gpg.rs`: `decrypt(ciphertext) -> SecretBytes` (zeroized)
- [x] `bypass-core::gpg_id`: resolve recipient list by walking `.gpg-id` up the tree
- [x] Unit tests against a throwaway GPG keyring (temp `GNUPGHOME`)

### Milestone 1.3: Core CRUD — `Store` in `bypass-core`, CLI dispatch in `bypass-cli`
- [x] `bypass-core::store`: resolve store root (`PASSWORD_STORE_DIR` or `~/.password-store`) via the Storage trait
- [x] `bypass init <gpg-id>` — write `.gpg-id`, optional `git init`
- [x] `bypass insert <path>` — read password from stdin/tty, encrypt, write
- [x] `bypass show <path>` — decrypt and print
- [x] `bypass ls [subpath]` — pretty tree of entries (rendering in CLI)
- [x] `bypass find <pattern>` — search entry names
- [x] `bypass doctor` — read-only check of environment (gpg, keyring, store root, .gpg-id, $EDITOR, git)
- [x] `bypass rm <path>` — delete entry (shred-style on `StorageFs`; see ADR-0008)
- [x] `bypass edit <path>` — decrypt to tempfile, open `$EDITOR`, re-encrypt
- [x] `bypass cp` / `bypass mv` — copy/move entries with re-encryption if needed

---

## Phase 2 — Git integration *(`vcs_git2` in `bypass-cli`)*

### Milestone 2.1: Repository management
- [x] `vcs_git2.rs`: init repo on `bypass init` when requested
- [x] Auto-commit on insert / edit / rm / cp / mv with meaningful messages
- [x] `bypass git ...` passthrough subcommand

### Milestone 2.2: History UX
- [x] `bypass log [path]` — show commit history for an entry
- [x] Handle dirty working tree gracefully (refuse, or stash)

---

## Phase 3 — Generation & clipboard

### Milestone 3.1: Password generation — `bypass-core`
- [x] `generate.rs`: cryptographically-secure password generation
- [x] Configurable length, symbol set, no-symbols flag
- [x] `bypass generate <path> [length]` — generate + store
- [x] `--in-place` to replace only the first line of an existing entry

### Milestone 3.2: Clipboard — `bypass-cli`
- [x] `clipboard.rs`: copy password via `arboard`
- [x] Auto-clear after N seconds (default 45), preserve prior clipboard contents
- [x] `bypass show -c <path>` and `bypass generate -c <path>`

---

## Phase 4 — Advanced entries

### Milestone 4.1: Structured entries — `bypass-core`
- [x] `entry.rs`: parse multi-line entries (first line = password, then `key: value`)
- [x] `bypass show <path> <field>` — print only one field
- [x] `-c` copy of a specific field

### Milestone 4.2: TOTP — `bypass-core`
- [x] `otp.rs`: parse `otpauth://` URIs in entries
- [x] `bypass otp <path>` — print current TOTP code
- [x] `bypass otp -c <path>` — copy TOTP code with auto-clear

### Milestone 4.3: Extensions — `bypass-cli`
- [x] `extensions.rs`: discover executables in `~/.password-store-extensions/`
- [x] Pass env vars (`PASSWORD_STORE_DIR`, etc.) to extensions
- [x] `bypass ext <name> [args]` dispatch

---

## Phase 5 — Sync

### Milestone 5.1: Git-based sync (default)
- [x] Document workflow: `bypass git remote add … && bypass git push`
- [x] `bypass sync` convenience command (pull + push)
- [x] Conflict resolution guidance
- [x] `bypass audit` + sync-time leak check refusing pushes that contain non-ciphertext files (see [ADR-0009](adr/0009-leak-check-before-push.md))

### Milestone 5.2: LAN P2P sync (stretch)
- [x] Design evaluation + transport / sync-semantics ADRs ([doc/sync-p2p-evaluation.md](sync-p2p-evaluation.md), [ADR-0010](adr/0010-p2p-transport-libp2p.md), [ADR-0011](adr/0011-sync-semantics-hybrid.md))
- [ ] **5.2.a** Device pairing flow (PAKE-from-PIN, peer-ID pinning)
- [ ] **5.2.b** Sync core: git pack over libp2p, hybrid auto-rebase policy, leak audit on receive
- [ ] **5.2.c** Daemon mode + `bypass sync status`
- [ ] **5.2.d** Two-peer integration tests + README rewrite

---

## Phase 6 — Polish

- [ ] `bypass completion <shell>` — generate shell completions
- [ ] Man page generation
- [ ] Migration helper from `pass` (should be a no-op if format matches)
- [ ] Integration tests covering full CRUD + git flows
- [ ] CI: build + test on Linux/macOS
- [ ] Release packaging (cargo-dist or similar)

---

## Phase 7 — Browser extension (Firefox + Chrome)

Strategy: thin WebExtension UI that delegates all crypto, storage, and git to the desktop `bypass` binary via the [WebExtension native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) protocol.

### Milestone 7.1: Native messaging host in the CLI
- [ ] `bypass messaging-host` subcommand — length-prefixed JSON on stdin/stdout
- [ ] JSON request schema: `ls`, `find`, `show`, `insert`, `generate`, `otp`
- [ ] Reuses the same `Store` instance as the CLI (same Crypto/Storage/VCS)
- [ ] Native-messaging manifest templates for Firefox and Chrome (install instructions)

### Milestone 7.2: WebExtension UI (Manifest V3)
- [ ] Single TypeScript codebase for Firefox + Chrome (`extension/` directory)
- [ ] `chrome.runtime.connectNative("io.bypass.host")` client
- [ ] Popup UI: search, reveal, copy-to-clipboard (via browser clipboard API)
- [ ] Optional in-page autofill on user gesture
- [ ] Packaging for AMO and Chrome Web Store

---

## Phase 8 — Android app

Strategy: native Jetpack Compose UI on top of `bypass-core` exposed via [UniFFI](https://mozilla.github.io/uniffi-rs/). The `Crypto` trait is declared as a UniFFI callback interface so the Kotlin side can back it with [OpenKeychain](https://github.com/open-keychain/open-keychain)'s OpenPGP AIDL service — the Rust core never holds keys.

### Milestone 8.1: FFI crate
- [ ] New `crates/bypass-ffi/` cdylib using UniFFI
- [ ] Declare `Crypto` as a callback (foreign-implemented) interface
- [ ] Expose `Store` operations through generated Kotlin bindings
- [ ] CI: build for `aarch64-linux-android` and `armv7-linux-androideabi`

### Milestone 8.2: Android UI
- [ ] Compose app shell, Material 3 theming
- [ ] OpenKeychain client (AIDL) implementing the Rust `Crypto` callback
- [ ] App-scoped storage backing the `Storage` trait
- [ ] Optional `git2` integration for sync (libgit2 with NDK), or defer to manual import/export

---

## Cross-cutting

- Never log decrypted content or write it outside tempfiles.
- All secrets in memory wrapped in `zeroize`-cleaned buffers.
- Every new dependency in `bypass-core` must compile on Android NDK and (eventually) `wasm32-unknown-unknown` if a future browser-side WASM bypass is reconsidered.
