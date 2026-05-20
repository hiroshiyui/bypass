---
name: docs-engineering
description: Audit and update all project documentation to stay in sync with the current development status.
---

When performing documentation engineering on `bypass`, always follow these steps:

1. **Survey recent changes** by running `git log --oneline -20` and skimming the diff of recent commits. This surfaces new features, removed dependencies, and behavioral changes that documentation may not yet reflect.

2. **Audit** all documentation against the current codebase and development status. The review scope must include — without exception:
   - `README.md` — once it exists: features list, prerequisites (`gpg` installed, Rust toolchain), install instructions, basic usage examples, acknowledgement of `pass` as inspiration.
   - `CHANGELOG.md` — once a first release is cut: release notes and version history in [Keep a Changelog](https://keepachangelog.com/) format.
   - `CLAUDE.md` — locked-in design decisions, build commands, gotchas, project conventions. Update when any decision changes.
   - `doc/ROADMAP.md` — **source of truth** for design and planned work. Tick checkboxes for completed items; do not delete them. Add new milestone items here when scope grows, after confirming with the user.
   - Rust doc comments (`///`) on public items in `store`, `gpg`, `git`, `entry`, `generate`, `clipboard`, `otp`, and `extensions` modules.

3. **Revise and update** any documentation that is stale, incomplete, or inconsistent with the current code. In particular:
   - When a milestone in `doc/ROADMAP.md` is completed, every checkbox in that milestone must be ticked.
   - When a locked-in decision changes (e.g., switching crypto backend), `CLAUDE.md` and `doc/ROADMAP.md` must both be updated in the same commit.
   - When a new command is added, its usage must be reflected in `README.md` (once present) and exposed via `clap`'s generated `--help`.

4. **Sync `doc/ROADMAP.md` with reality** — if work has landed that isn't represented in the roadmap, add the corresponding (already-ticked) checkbox so the roadmap remains a faithful log. If a planned item was abandoned, remove it with a one-line note in the commit message explaining why.

5. **Commit** documentation changes using the `commit-and-push` skill, grouped by topic. Do not mix unrelated documentation changes in a single commit.
