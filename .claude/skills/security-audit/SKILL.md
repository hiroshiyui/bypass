---
name: security-audit
description: Perform project-wide security audits of the bypass password manager.
---

`bypass` is a password manager — security review is the highest-stakes review this project has. When auditing, always follow these steps:

1. **Dependency audit**
   - Run `cargo audit` (install with `cargo install cargo-audit` if missing) against `Cargo.lock`. Any unpatched RUSTSEC advisory in a dependency that touches crypto, parsing, networking, or process spawning is treated as **Critical** until proven otherwise.
   - Run `cargo deny check` if `deny.toml` is configured. Confirm no GPL/AGPL deps were pulled in transitively (the crate is intended to be permissively licensed).
   - Flag any new dependency that surprises you for a password manager — supply-chain risk grows with each crate.

2. **Plaintext lifecycle**
   - Trace every place a decrypted secret enters memory. Confirm it lives in a `Zeroizing<Vec<u8>>` / `Zeroizing<String>` (or equivalent) and is dropped as early as possible.
   - Confirm no decrypted value is ever passed to `format!`, `println!`, `eprintln!`, `log::*`, `tracing::*`, `dbg!`, or a panic message.
   - For `bypass edit`: the tempfile must be created with `0600` perms on a path the user controls (prefer `XDG_RUNTIME_DIR` / tmpfs); the file must be unlinked before the editor exits, and the buffer zeroized.

3. **GPG subprocess hygiene**
   - All `gpg` invocations use `std::process::Command` with arguments as separate `arg()` calls — no shell, no string concatenation.
   - Plaintext enters `gpg` only via stdin (`Stdio::piped()`); ciphertext leaves via stdout. Never write plaintext to a tempfile that `gpg` then reads.
   - The child process is waited on; non-zero exit codes propagate as errors, not silent fallbacks.
   - `GNUPGHOME` is respected when set; tests never inherit the user's real `GNUPGHOME`.

4. **Filesystem boundary**
   - Every entry name is canonicalized against the resolved store root. Reject `..`, absolute paths, and resolved paths that escape the root.
   - Symlink handling: refuse to follow symlinks that point outside the store. `.gpg-id` lookup must use the resolved, post-symlink path.
   - File modes: `.gpg` files and `.gpg-id` should be `0600` / `0644` respectively; the store root should be `0700`.

5. **Git integration risk**
   - `git2` operations must not leak plaintext into commit messages, refs, or notes.
   - When a commit fails, the corresponding filesystem mutation must be rolled back (or never made — write-then-commit ordering matters).
   - Auto-commit must not run inside an interactive merge/rebase state — detect and refuse.

6. **Clipboard handling**
   - Auto-clear must execute on SIGINT, SIGTERM, and panic paths — not only on the happy path.
   - The previous clipboard contents are restored (not erased) when the secret expires.
   - On Wayland/X11/macOS, confirm `arboard` behavior matches expectations on each platform actually targeted.

7. **Generation entropy**
   - Password generation uses `OsRng` (via `rand::rngs::OsRng` or equivalent CSPRNG). Never use `thread_rng()` for secrets unless it is documented as CSPRNG-backed for the chosen crate version.
   - TOTP secret material is treated as plaintext — same zeroization rules apply.

8. **Sync layer (when present)**
   - Confirm only encrypted `.gpg` blobs cross the network. The sync layer must not have access to GPG keys or plaintext.
   - Authentication between peers must be cryptographic, not LAN-trust-based.

9. **Report findings** — Document all identified risks, classify by severity (**Critical**, **High**, **Medium**, **Low**), and provide specific remediation steps. Include file path + line number for every finding. For Critical issues, also suggest whether a hotfix release is warranted.
