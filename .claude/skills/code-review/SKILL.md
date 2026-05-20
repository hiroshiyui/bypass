---
name: code-review
description: Perform a project-wide code review covering security, correctness, code quality, tests, documentation, and style.
---

When performing a project-wide code review of `bypass`, always follow these steps:

1. **Survey recent changes** — Run `git log --oneline -20` and skim the corresponding diffs to understand the scope of work before examining individual files. Cross-reference against `doc/ROADMAP.md` to confirm the change belongs to the current phase/milestone.

2. **Security audit** — Apply the `security-audit` skill. Give particular attention to:
   - *Plaintext handling:* decrypted secrets must live in `zeroize`'d buffers and never reach `println!`, `dbg!`, `log`, panic messages, or any non-tempfile path. Tempfiles used by `edit` must live on a tmpfs / `O_TMPFILE` path where possible and be unlinked before the editor exits.
   - *GPG subprocess invocation:* arguments must be passed as a `Vec<&str>` (never a shell string); stdin is the only acceptable channel for plaintext entering `gpg`; environment must respect `GNUPGHOME` and not leak through to child processes that don't need it.
   - *Path traversal:* every entry name from the user must be resolved against the store root and rejected if it escapes (`..`, absolute paths, symlinks pointing outside the store).
   - *Recipient resolution:* `.gpg-id` lookup must walk *up* from the entry path and stop at the store root — never silently fall through to a default key.
   - *Clipboard auto-clear:* the clear path must run even on SIGINT / panic; the prior clipboard contents must be restored, not blanked.

3. **Correctness and logic** — Review the Rust implementation for:
   - *Pass compatibility:* on-disk layout must match `pass` — `<path>/<name>.gpg`, `.gpg-id` files, optional `.gpg-id.sig`. A store created by `pass` must be readable by `bypass` and vice-versa.
   - *Error propagation:* `anyhow::Result` at command boundaries, typed `thiserror` errors for library modules (`gpg`, `store`, `git`). No `unwrap()` / `expect()` in non-test code where a meaningful error could be propagated.
   - *Git integration:* every mutation (`insert`, `edit`, `rm`, `cp`, `mv`, `generate`) must produce exactly one commit with a meaningful message; failures must leave the working tree in a consistent state (no half-written `.gpg` file without a matching commit).
   - *2024 edition idioms:* the crate targets edition 2024 — flag any patterns copied from older examples that won't compile (e.g., outdated `gen` keyword usage, lifetime elision changes).

4. **Code smells** — Flag any of the following:
   - Duplicated logic that should be extracted into a shared helper.
   - Functions exceeding roughly 60 lines without clear justification.
   - Magic numbers (clipboard clear timeout, generated password length, etc.) appearing in multiple places without a named constant.
   - Hard-coded paths instead of `dirs::home_dir()` / `PASSWORD_STORE_DIR` resolution.
   - Dead code or stale commented-out blocks.

5. **Test coverage** — Verify that:
   - New logic has unit tests co-located in the same file under `#[cfg(test)] mod tests`.
   - Tests touching GPG set a throwaway `GNUPGHOME` via `tempfile::TempDir` and never read or write the user's real keyring.
   - Tests touching the store set `PASSWORD_STORE_DIR` to a tempdir, not `~/.password-store`.
   - Integration tests live under `tests/` and exercise full command flows (e.g., `init` → `insert` → `show` round-trip).

6. **Documentation quality** — Confirm that:
   - Public items in `store`, `gpg`, `git`, and `entry` modules carry `///` doc comments.
   - `CLAUDE.md` is updated for any new locked-in decision or gotcha.
   - `doc/ROADMAP.md` checkboxes are ticked for completed milestone items.

7. **CLI UX** — Review clap definitions for:
   - Subcommand names and flags matching `pass` where the semantics match (`-c` for clipboard, `--in-place` for generate, etc.) — divergence requires a written reason.
   - Help text that names the entry path argument consistently (e.g., always `<path>`, never sometimes `<name>`).
   - Confirmation prompts on destructive operations (`rm`, overwrite on `insert`).

8. **Code style** — Confirm that formatting rules are observed:
   - Code must be `cargo fmt`-clean.
   - `cargo clippy --all-targets -- -D warnings` must pass.
   - Any `#[allow(clippy::...)]` suppression must be accompanied by a comment explaining why the lint is a false positive in that context.

9. **Report findings** — Present all identified issues grouped by category: Security, Correctness, Code Smell, Tests, Documentation, CLI UX, and Style. Assign each a severity of **Critical**, **High**, **Medium**, or **Low**. For every finding, include the file path and line number, a clear description of the problem, and a concrete recommendation for how to fix it.
