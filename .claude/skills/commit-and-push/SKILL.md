---
name: commit-and-push
description: Stage, commit, and push changes to the remote repository with a well-formed commit message.
---

When committing and pushing changes, always follow these steps:

1. **Stage** all relevant changes with `git add`. Be deliberate — stage only files related to the current topic. Never blindly stage everything with `git add -A` if unrelated changes are present. In particular, never stage:
   - Anything under `target/`
   - `.gpg` files, `.gpg-id` files, or anything that looks like real key material — these belong in a user's store, not this repo
   - Local editor / IDE config not already tracked

2. **Run pre-commit checks** before committing:
   - `cargo fmt --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo test` for the affected area (full suite if the change is non-local)
   Fix any failures before proceeding rather than committing broken code.

3. **Commit** with a clear, concise message following the [Conventional Commits](https://www.conventionalcommits.org/) standard. Typical scopes for this project: `cli`, `store`, `gpg`, `git`, `entry`, `generate`, `clipboard`, `otp`, `ext`, `sync`, `docs`, `build`. Examples:
   - `feat(gpg): resolve recipients by walking .gpg-id up the tree`
   - `fix(store): reject entry paths that escape the store root`
   - `docs(roadmap): tick milestone 1.2 checkboxes`
   The message body should explain *why* the change was made, not just *what* changed.

4. **Push** the committed changes to the current branch on the remote repository (if a remote is configured).

5. **Verify** that the push succeeded and the remote is in sync with the local branch.
