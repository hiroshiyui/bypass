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
   - `doc/adr/` — Architecture Decision Records. Audit that every load-bearing decision in the codebase has a corresponding ADR, that ADR statuses are accurate (no `accepted` ADR should describe code that has since been ripped out), and that `doc/adr/README.md`'s index table lists every file under `doc/adr/` with the correct title and current status.
   - Rust doc comments (`///`) on public items in `store`, `gpg`, `git`, `entry`, `generate`, `clipboard`, `otp`, and `extensions` modules.

3. **Revise and update** any documentation that is stale, incomplete, or inconsistent with the current code. In particular:
   - When a milestone in `doc/ROADMAP.md` is completed, every checkbox in that milestone must be ticked.
   - When a locked-in decision changes (e.g., switching crypto backend), `CLAUDE.md` and `doc/ROADMAP.md` must both be updated in the same commit, **and** a new ADR must be written under `doc/adr/` (see step 5 below).
   - When a new command is added, its usage must be reflected in `README.md` (once present) and exposed via `clap`'s generated `--help`.

4. **Sync `doc/ROADMAP.md` with reality** — if work has landed that isn't represented in the roadmap, add the corresponding (already-ticked) checkbox so the roadmap remains a faithful log. If a planned item was abandoned, remove it with a one-line note in the commit message explaining why.

5. **Maintain `doc/adr/`** following the conventions defined in [ADR-0000](../../doc/adr/0000-record-architecture-decisions.md):
   - **Never rewrite an accepted ADR.** Edits to merged ADRs are limited to status changes (e.g. `accepted` → `superseded by [ADR-XXXX](XXXX-…​.md)`) and adding cross-links to newer ADRs. To reverse or significantly extend a recorded decision, write a *new* ADR that supersedes the old one and flip the old one's status — do not silently change history.
   - **Write a new ADR whenever a load-bearing decision is made or changed**, including: dependency swaps that affect the architecture (e.g. `git2` → `gix`), changes to the trait surface in `bypass-core`, changes to the on-disk store layout, new platform targets, or licence changes. Trivial dependency bumps and bug fixes do not warrant an ADR.
   - **Filename convention:** `NNNN-kebab-title.md`, four-digit zero-padded sequence number, allocated in commit order, **never reused** — even for rejected or superseded ADRs. The next number is one greater than the highest number currently in `doc/adr/`.
   - **Required structure (MADR 4.x):** SPDX header comment; title; metadata block with `Status`, `Date`, `Deciders`; `Context and Problem Statement`; `Considered Options`; `Decision Outcome` (chosen option with rationale); `Consequences` (good and bad); and a `Confirmation` section pointing at the code, test, or CI check that enforces the decision.
   - **Status vocabulary:** `proposed`, `accepted`, `rejected`, `deprecated`, or `superseded by [ADR-XXXX](XXXX-…​.md)`.
   - **Keep `doc/adr/README.md` in sync.** Every new ADR must be added to its index table, and any status change must be reflected there in the same commit.
   - **Cross-link aggressively.** When a new ADR builds on, contrasts with, or supersedes an existing one, link both ways: add a reference in the new ADR's body *and* update the older ADR's status / "see also" section.

6. **Commit** documentation changes using the `commit-and-push` skill, grouped by topic. Do not mix unrelated documentation changes in a single commit. ADR additions are typically their own commit with the `docs(adr):` scope.
