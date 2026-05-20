# bypass — Roadmap

A CLI password manager in Rust, inspired by [pass](https://www.passwordstore.org/).

## Design decisions

- **Crypto backend:** shell out to `gpg` (pass-compatible stores, easy migration).
- **Versioning:** git-backed via the `git2` crate.
- **Storage layout:** mirrors pass — `~/.password-store/<path>/<name>.gpg`, with `.gpg-id` files marking recipient keys per subtree.
- **Sync strategy:** start with git remotes (push/pull over SSH or shared LAN path); evaluate Syncthing-style P2P only after core is stable.

## Proposed crate layout

```
bypass/
├── Cargo.toml
└── src/
    ├── main.rs            # clap CLI dispatch
    ├── cli.rs             # command definitions (clap derive)
    ├── store.rs           # PasswordStore: paths, traversal, ls/find
    ├── gpg.rs             # encrypt/decrypt via gpg subprocess
    ├── git.rs             # git2 wrappers: init, commit, log
    ├── entry.rs           # multi-line entry parsing (password + fields)
    ├── generate.rs        # password generation (rand)
    ├── clipboard.rs       # arboard + auto-clear
    ├── otp.rs             # TOTP (totp-rs crate)
    ├── extensions.rs      # discover & exec extensions
    └── sync/              # later phase
```

Core dependencies: `clap`, `git2`, `arboard`, `rand`, `totp-rs`, `anyhow`, `thiserror`, `dirs`, `zeroize`.

---

## Phase 1 — Foundations

### Milestone 1.1: Project skeleton
- [ ] Flesh out `Cargo.toml` with core dependencies
- [ ] Set up `cli.rs` with clap derive command enum
- [ ] Wire `main.rs` to dispatch subcommands
- [ ] Add `anyhow`/`thiserror` error scaffolding
- [ ] Add `.gitignore` entries for `target/`, secrets in tests

### Milestone 1.2: GPG crypto path
- [ ] `gpg.rs`: `encrypt(plaintext, recipients) -> Vec<u8>`
- [ ] `gpg.rs`: `decrypt(ciphertext) -> Vec<u8>` (zeroized)
- [ ] Resolve `.gpg-id` recipient lookup walking up the tree
- [ ] Unit tests against a throwaway GPG keyring

### Milestone 1.3: Core CRUD
- [ ] `store.rs`: resolve store root (`PASSWORD_STORE_DIR` or `~/.password-store`)
- [ ] `bypass init <gpg-id>` — write `.gpg-id`, optional `git init`
- [ ] `bypass insert <path>` — read password from stdin/tty, encrypt, write
- [ ] `bypass show <path>` — decrypt and print
- [ ] `bypass ls [subpath]` — pretty tree of entries
- [ ] `bypass find <pattern>` — search entry names
- [ ] `bypass rm <path>` — delete entry
- [ ] `bypass edit <path>` — decrypt to tempfile, open `$EDITOR`, re-encrypt
- [ ] `bypass cp` / `bypass mv` — copy/move entries with re-encryption if needed

---

## Phase 2 — Git integration

### Milestone 2.1: Repository management
- [ ] `git.rs`: init repo on `bypass init` when requested
- [ ] Auto-commit on insert / edit / rm / cp / mv with meaningful messages
- [ ] `bypass git ...` passthrough subcommand

### Milestone 2.2: History UX
- [ ] `bypass log [path]` — show commit history for an entry
- [ ] Handle dirty working tree gracefully (refuse, or stash)

---

## Phase 3 — Generation & clipboard

### Milestone 3.1: Password generation
- [ ] `generate.rs`: cryptographically-secure password generation
- [ ] Configurable length, symbol set, no-symbols flag
- [ ] `bypass generate <path> [length]` — generate + store
- [ ] `--in-place` to replace only the first line of an existing entry

### Milestone 3.2: Clipboard
- [ ] `clipboard.rs`: copy password via `arboard`
- [ ] Auto-clear after N seconds (default 45), preserve prior clipboard contents
- [ ] `bypass show -c <path>` and `bypass generate -c <path>`

---

## Phase 4 — Advanced entries

### Milestone 4.1: Structured entries
- [ ] `entry.rs`: parse multi-line entries (first line = password, then `key: value`)
- [ ] `bypass show <path> <field>` — print only one field
- [ ] `-c` copy of a specific field

### Milestone 4.2: TOTP
- [ ] `otp.rs`: parse `otpauth://` URIs in entries
- [ ] `bypass otp <path>` — print current TOTP code
- [ ] `bypass otp -c <path>` — copy TOTP code with auto-clear

### Milestone 4.3: Extensions
- [ ] `extensions.rs`: discover executables in `~/.password-store-extensions/`
- [ ] Pass env vars (`PASSWORD_STORE_DIR`, etc.) to extensions
- [ ] `bypass ext <name> [args]` dispatch

---

## Phase 5 — Sync

### Milestone 5.1: Git-based sync (default)
- [ ] Document workflow: `bypass git remote add … && bypass git push`
- [ ] `bypass sync` convenience command (pull + push)
- [ ] Conflict resolution guidance

### Milestone 5.2: LAN P2P sync (stretch)
- [ ] Evaluate `libp2p` (mDNS + noise + gossipsub) vs custom protocol
- [ ] Device pairing flow (QR / short code)
- [ ] Encrypted-at-rest blobs only — sync layer never sees plaintext
- [ ] Conflict resolution (per-entry last-write-wins with history retained in git)
- [ ] Daemon mode + `bypass sync status`

---

## Phase 6 — Polish

- [ ] `bypass completion <shell>` — generate shell completions
- [ ] Man page generation
- [ ] Migration helper from `pass` (should be a no-op if format matches)
- [ ] Integration tests covering full CRUD + git flows
- [ ] CI: build + test on Linux/macOS
- [ ] Release packaging (cargo-dist or similar)
