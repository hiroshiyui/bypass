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
    │       ├── crypto.rs            # Crypto trait + SecretBytes + KeyId
    │       ├── storage.rs           # Storage trait
    │       ├── vcs.rs               # VersionControl trait (+ NoVcs)
    │       ├── path.rs              # RelPath newtype, traversal-safe
    │       ├── gpg_id.rs            # walk-up .gpg-id recipient resolution
    │       ├── store.rs             # Store<C,S,V> orchestrator + canonical .gitattributes
    │       ├── entry.rs             # multi-line entry parsing
    │       ├── generate.rs          # password generation
    │       ├── otp.rs               # TOTP from otpauth:// URIs
    │       └── error.rs             # shared error scaffolding
    └── bypass-cli/                  # Linux binary `bypass`
        ├── src/
        │   ├── main.rs              # clap dispatch
        │   ├── cli.rs               # clap derive command enum
        │   ├── crypto_gpg.rs        # impl Crypto via `gpg` subprocess
        │   ├── storage_fs.rs        # impl Storage on local FS (shred-on-remove)
        │   ├── vcs_git2.rs          # impl VersionControl via libgit2
        │   ├── audit.rs             # leak-check audit (ADR-0009)
        │   ├── clipboard.rs         # arboard + auto-clear daemon
        │   ├── doctor.rs            # `bypass doctor` env probe
        │   ├── edit.rs              # `bypass edit` tempfile + $EDITOR
        │   ├── extensions.rs        # pass-style extension dispatch
        │   ├── tree.rs              # ASCII tree renderer for `bypass ls`
        │   └── sync/                # Phase 5.2 LAN P2P sync
        │       ├── mod.rs
        │       ├── identity.rs      # Ed25519 identity key (ADR-0015)
        │       ├── peers.rs         # peers.toml pinned-peer table (ADR-0012)
        │       ├── transport.rs     # Transport trait + InProcessTransport (ADR-0013)
        │       ├── libp2p_transport.rs  # real libp2p Transport (ADR-0010)
        │       ├── pairing.rs       # SPAKE2 PAKE-from-PIN (ADR-0012)
        │       ├── wire.rs          # WantPackFrom / Pack / Err wire types
        │       ├── syncing.rs       # pack build/ingest + reconcile (ADR-0011/0014/0016)
        │       ├── merge_driver.rs  # `bypass-take-theirs` (ADR-0011)
        │       ├── ratelimit.rs     # per-peer attempt window (ADR-0016)
        │       ├── socket.rs        # daemon status socket (ADR-0017/0018)
        │       ├── watcher.rs       # notify-based fs watcher
        │       └── daemon.rs        # `bypass sync daemon` main loop
        └── tests/
            ├── common/mod.rs        # throwaway GNUPGHOME + TestEnv helper
            ├── end_to_end.rs        # default-suite integration tests
            ├── sync_loopback.rs     # #[ignore]: two-process pair via libp2p
            └── sync_daemon.rs       # #[ignore]: daemon lifecycle + mDNS round-trip
```

Reserved for later phases: `crates/bypass-ffi/` (UniFFI surface for Android) and `extension/` (TypeScript WebExtension for Firefox & Chrome).

Actual dependencies as of Phase 5.2:
- `bypass-core`: `rand`, `totp-rs`, `thiserror`, `zeroize` (portability rule
  forbids `git2`, `arboard`, `libp2p`, `tokio`, `notify`, or any subprocess crate).
- `bypass-cli`: `anyhow`, `arboard`, `clap`, `dirs`, `git2`, `libp2p` (+ `libp2p-identity`),
  `notify`, `rand`, `rpassword`, `serde`, `serde_json`, `sha2`, `spake2`,
  `thiserror`, `tokio`, `toml`.

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

### Milestone 4.4: Backup, migration, and GPG key rotation ([ADR-0026](adr/0026-export-import-for-backup-and-rotation.md))
- [ ] **4.4.a** Tar packing/unpacking + manifest schema (with format version field) in `bypass-core`; no I/O dependencies, plaintext held in `SecretBytes` between read and tar-write
- [ ] **4.4.b** `bypass backup --to <recipient> [--subtree <path>]` in `bypass-cli` — decrypts each entry, streams plaintext tar through `gpg --encrypt --recipient <recipient>` to stdout; one entry's plaintext in RAM at a time
- [ ] **4.4.c** `bypass restore <bundle>` (fresh-store mode) — requires destination initialised by `bypass init <new-key>` and empty; decrypts outer tar via `gpg`, re-encrypts each entry to the destination's `.gpg-id`, applies `storage_fs::overwrite_then_unlink` ([ADR-0008](adr/0008-secure-delete-via-overwrite.md)) on any prior file at the target path
- [ ] **4.4.d** `bypass restore --in-place <bundle>` — rewrites `.gpg-id` first, then re-encrypts every entry, wrapped in a single `Re-encrypt store for <new-key>` commit so paired peers can fast-forward without ancestry breakage ([ADR-0011](adr/0011-sync-semantics-hybrid.md), [ADR-0014](adr/0014-sync-metadata-and-ordering.md))
- [ ] **4.4.e** Round-trip integration tests in `crates/bypass-cli/tests/end_to_end.rs` — key-A→key-B `backup`+fresh-`restore` asserts every entry decrypts to the same plaintext; a second test exercises `--in-place` and asserts the git log shows the single rewrite commit with prior ancestry intact
- [ ] **4.4.f** Help-text and README docs covering the forward-confidentiality caveat (old ciphertext an attacker exfiltrated stays readable if the old private key leaks; rotate the underlying *passwords* for entries that matter), the git-history caveat (prior commits retain old ciphertexts; users wanting them scrubbed need `git filter-repo`/BFG — bypass does not ship a history-rewriting command), and the verb distinction (`backup`/`restore` move bypass-native bundles; `import` ingests foreign vaults — see [ADR-0027](adr/0027-foreign-format-importers.md))
- [ ] **4.4.g** *(stretch)* `bypass doctor` warning when `.gpg-id` names a key whose primary algorithm is RSA-1024 or DSA, nudging toward rotation

### Milestone 4.5: Foreign-format importers ([ADR-0027](adr/0027-foreign-format-importers.md))
- [ ] **4.5.a** `ImportedEntry` type + canonical mapping logic in `bypass-core::import` — slugging rules, `login:`/`url:`/`url-N:`/`otpauth:` key conventions, in-batch collision suffixing, store-collision atomic-fail (matching the rule [ADR-0026](adr/0026-export-import-for-backup-and-rotation.md) set for `restore`); pure logic, no I/O
- [ ] **4.5.b** Bitwarden parser in `bypass-core::import::bitwarden` — plain JSON export (`bitwarden_export.json`); separately, encrypted JSON (`bitwarden_encrypted_export.json`) decryption gated on master password read via dedicated fd (not echo'd, held in `SecretBytes`)
- [ ] **4.5.c** KeePass KDBX-XML parser in `bypass-core::import::keepass` — group hierarchy → subtree paths, entries' standard fields + custom string fields → `ImportedEntry`
- [ ] **4.5.d** Generic RFC-4180 CSV parser in `bypass-core::import::csv` with `--csv-schema=<header-spec>` to map columns to `password`/`username`/`url`/notes (no automagic header sniffing — users state their schema explicitly)
- [ ] **4.5.e** `bypass import --format=<bitwarden|keepass|csv> <file>` dispatch in `bypass-cli`; per-entry encrypt-and-commit funnels through the same write path as 4.4.c's `restore` (one write path, see ADR-0027 reasoning)
- [ ] **4.5.f** `bypass import --from-ext <name> <file>` — invokes `bypass-import-<name>` extension via the existing `bypass ext` mechanism, captures its stdout (an [ADR-0026](adr/0026-export-import-for-backup-and-rotation.md) bundle), feeds it through the restore path; the bundle stays an internal IPC contract, not a user-facing surface
- [ ] **4.5.g** Mandatory lossiness summary on stderr at end of every import — list of fields dropped or transformed (collapsed newlines, unsupported attachment counts, custom-field types coerced), so users see what didn't survive *before* deleting the source vault
- [ ] **4.5.h** Integration tests in `crates/bypass-cli/tests/end_to_end.rs` — Bitwarden JSON fixture and KeePass KDBX-XML fixture under `tests/fixtures/`, each round-tripping through `bypass import --format=…` into a throwaway store with assertions on entry paths, decrypted passwords, and a representative custom field; a separate test drives a tiny in-repo stub extension to confirm the `--from-ext` path works end-to-end
- [ ] **4.5.i** `doc/extensions/importer-protocol.md` documenting the extension contract — stdin/stdout shape, exit codes, the ADR-0026 bundle as the wire format, and a worked example skeleton in shell or Python

---

## Phase 5 — Sync

### Milestone 5.1: Git-based sync (default)
- [x] Document workflow: `bypass git remote add … && bypass git push`
- [x] `bypass sync` convenience command (pull + push)
- [x] Conflict resolution guidance
- [x] `bypass audit` + sync-time leak check refusing pushes that contain non-ciphertext files (see [ADR-0009](adr/0009-leak-check-before-push.md))

### Milestone 5.2: LAN P2P sync (stretch)
- [x] Design evaluation + transport / sync-semantics ADRs ([doc/sync-p2p-evaluation.md](sync-p2p-evaluation.md), [ADR-0010](adr/0010-p2p-transport-libp2p.md), [ADR-0011](adr/0011-sync-semantics-hybrid.md))
- [x] **5.2.a** Device pairing flow (PAKE-from-PIN, peer-ID pinning) — SPAKE2 handshake + identity / `peers.toml` persistence + `Transport` trait + `InProcessTransport` + `bypass sync identity rotate`; `bypass sync pair` clap surface staged for 5.2.b's libp2p wiring
- [x] **5.2.b** Sync core: git pack over libp2p, hybrid auto-rebase policy, leak audit on receive
  - [x] **5.2.b.i** `Libp2pTransport` (real network) + `bypass sync pair --show/--enter` over libp2p ([ADR-0010](adr/0010-p2p-transport-libp2p.md))
  - [x] **5.2.b.ii** Sync core: `WantPackFrom` RPC, custom merge driver, leak-audit on receive
  - [x] **5.2.b.iii** ADR-0016 (DoS defences: pack-size cap + rate limit) + bootstrap-protocol verification + two-process integration tests
- [x] **5.2.c** Daemon mode + `bypass sync status` ([ADR-0017](adr/0017-daemon-socket-location.md), [ADR-0018](adr/0018-daemon-status-protocol.md), [ADR-0019](adr/0019-peer-revocation-trust-semantics.md))
- [x] **5.2.d** Two-peer integration tests + README rewrite

---

## Phase 6 — Polish

- [x] `bypass completion <shell>` — generate shell completions
- [x] Man page generation (`bypass man`)
- [x] Migration helper from `pass` (no-op — same on-disk format per [ADR-0002](adr/0002-pass-compatible-on-disk-layout.md); see README "Migrating from `pass`")
- [x] Integration tests covering full CRUD + git flows ([`tests/end_to_end.rs`](../crates/bypass-cli/tests/end_to_end.rs): 36 tests; [`tests/sync_loopback.rs`](../crates/bypass-cli/tests/sync_loopback.rs) + [`tests/sync_daemon.rs`](../crates/bypass-cli/tests/sync_daemon.rs) `#[ignore]`-by-default)
- [x] CI: build + test on Linux ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml); macOS dropped per [ADR-0028](adr/0028-drop-macos-support.md))
- [x] Release packaging ([`.github/workflows/release.yml`](../.github/workflows/release.yml) + [ADR-0021](adr/0021-release-packaging.md): hand-rolled, two Linux targets on `v*` tags; the two darwin targets are removed per [ADR-0028](adr/0028-drop-macos-support.md))
- [x] Sync-daemon service integration (`install` / `uninstall` / `start` / `stop` / `enable` / `disable` / `status`, per [ADR-0020](adr/0020-daemon-service-supervision.md)):
  - [x] Linux: systemd user unit at `~/.config/systemd/user/bypass-sync.service`, managed via `systemctl --user`
  - ~~macOS: launchd agent at `~/Library/LaunchAgents/io.bypass.sync.plist`~~ removed per [ADR-0028](adr/0028-drop-macos-support.md)
  - Resolves the Phase 5.2 daemon-supervision open question recorded in [`doc/sync-p2p-evaluation.md`](sync-p2p-evaluation.md)
- [x] **CLI workflow eval** — exercised the full surface against a throwaway keyring; closed seven findings (commits `52f6349` + `e507a43`):
  - [x] **F1** `bypass init` refuses to overwrite an existing `.gpg-id` unless `--force`
  - [x] **F2** `bypass insert` refuses zero-byte plaintext
  - [x] **F3** `init` / `insert` / `generate` emit stderr confirmations (`added` / `updated` / `rotated` / `initialised store …`) matching the existing `cp` / `mv` / `rm` style
  - [x] **F4** `messaging-host --help` no longer leaks an unrendered Markdown ADR link
  - [x] **F5** `bypass find` with no matches exits 1 with a stderr message (was: silent exit 0)
  - [x] **F6** `bypass git` passthrough soft-warns before known-destructive shapes (`reset --hard`, `clean -f*`, `checkout .` / `--`, `branch -D`, `push --force[-with-lease]`); never refuses
  - [x] **F7** `bypass show -c` (and friends) probe `arboard::Clipboard::new()` in the foreground before claiming success — tty-only sessions get a clear "install xclip/xsel/wl-clipboard" error instead of a silent daemon death

---

## Phase 7 — Browser extension (Firefox + Chrome)

Strategy: thin WebExtension UI that delegates all crypto, storage, and git to the desktop `bypass` binary via the [WebExtension native messaging](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) protocol.

### Milestone 7.1: Native messaging host in the CLI ([ADR-0022](adr/0022-native-messaging-wire-protocol.md))
- [x] `bypass messaging-host` subcommand — length-prefixed JSON on stdin/stdout
- [x] JSON request schema: `ls`, `find`, `show`, `insert`, `generate`, `otp`, `rm`
- [x] Reuses the same `Store` instance as the CLI (same Crypto/Storage/VCS)
- [x] Native-messaging manifest templates for Firefox and Chrome (via `bypass messaging-host install`)

### Milestone 7.2: WebExtension UI (Manifest V3, [ADR-0023](adr/0023-browser-extension-architecture.md))
- [x] Single TypeScript codebase for Firefox + Chrome ([`extension/`](../extension/))
- [x] `chrome.runtime.connectNative("io.bypass.host")` client ([`extension/src/native.ts`](../extension/src/native.ts))
- [x] Popup UI: search, reveal, copy-to-clipboard (via browser clipboard API)
- [ ] **7.2.b** Optional in-page autofill on user gesture (deferred — needs content scripts + background worker)
- [ ] **7.2.b** Packaging for AMO and Chrome Web Store (build script emits a loadable / submittable zip; manual upload for v1)

---

## Phase 8 — Android app

Strategy: native Jetpack Compose UI on top of `bypass-core` exposed via [UniFFI](https://mozilla.github.io/uniffi-rs/). The `Crypto` trait is declared as a UniFFI callback interface so the Kotlin side can back it with [OpenKeychain](https://github.com/open-keychain/open-keychain)'s OpenPGP AIDL service — the Rust core never holds keys.

### Milestone 8.1: FFI crate ([ADR-0024](adr/0024-android-ffi-via-uniffi.md))
- [x] New [`crates/bypass-ffi/`](../crates/bypass-ffi/) cdylib using UniFFI
- [x] Declare `Crypto` as a callback (foreign-implemented) interface
- [x] Expose `Store` operations through generated Kotlin bindings (`cargo run -p bypass-ffi --bin uniffi-bindgen` emits at build time; the Android Gradle project in 8.2 invokes it)
- [x] CI: build for `aarch64-linux-android` and `armv7-linux-androideabi` ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml) `android-ffi` job)

### Milestone 8.2: Android UI ([ADR-0025](adr/0025-android-ui-architecture.md))
- [x] **8.2.a** Compose app shell, Material 3 theming ([`android/`](../android/))
- [x] **8.2.a** App-scoped storage backing the `Storage` trait (`context.filesDir.resolve("store")` via `bypass-ffi::AppStorage`)
- [x] **8.2.b** OpenKeychain client (AIDL) implementing the Rust `Crypto` callback ([`OpenKeychainCrypto.kt`](../android/app/src/main/kotlin/io/bypass/android/crypto/OpenKeychainCrypto.kt)) — happy-path scope; cold-cache surfaces an actionable `BypassException.Crypto` error
- [x] **8.2.b** Android Gradle build in CI ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml) `android-gradle-build` job)
- [x] **8.2.b.ii** Async PendingIntent bridge for `RESULT_CODE_USER_INTERACTION_REQUIRED` ([`CryptoUiBridge.kt`](../android/app/src/main/kotlin/io/bypass/android/crypto/CryptoUiBridge.kt) + `MainActivity`'s `ActivityResultLauncher`) — auto-launches OpenKeychain on cold-cache, resumes the FFI call when the user confirms; bounded by 5 interaction rounds per op
- [ ] **8.2.c** Optional `git2` integration for sync (libgit2 with NDK), or defer to manual import/export

---

## Cross-cutting

- Never log decrypted content or write it outside tempfiles.
- All secrets in memory wrapped in `zeroize`-cleaned buffers.
- Every new dependency in `bypass-core` must compile on Android NDK and (eventually) `wasm32-unknown-unknown` if a future browser-side WASM bypass is reconsidered.
- Stores carry a `.gitattributes` with `*.gpg binary` to disable line-ending normalisation on cross-platform clones (Windows `core.autocrlf` would otherwise corrupt ciphertext). Written by `bypass init`; lazily installed by `bypass sync` on stores that pre-date the rule; surfaced by `bypass doctor`. Phase 5.2.b's merge driver extends — does not replace — this line.
