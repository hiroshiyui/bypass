<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Use the `git2` crate (libgit2) for versioning, not subprocess `git`

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass` versions a password store with git: every mutation
(`insert`/`edit`/`rm`/`cp`/`mv`) auto-commits, and the user can `bypass git
…` for arbitrary plumbing. The CLI needs a way to talk to git.

This sits in contrast with ADR-0001, where we chose to shell out to `gpg`.
The two cases look superficially similar, but the trade-offs go opposite
directions.

## Considered Options

* **Subprocess `git`** (consistent with the `gpg` path).
* **[`git2`](https://crates.io/crates/git2)** — Rust bindings to
  [libgit2](https://libgit2.org/).
* **`gix`** — pure-Rust reimplementation.

## Decision Outcome

Chosen option: **`git2` crate**, in `bypass-cli` only.

* Unlike `gpg`, the user does not need their *own* git binary, keyring, or
  agent: there is no "the system git is already set up the way the user
  wants" surface to preserve.
* `git2` gives us strongly-typed access to repository state (index,
  workdir, refs) without parsing porcelain. Auto-commits driven by file
  mutations are much cleaner with a real API than by spawning processes.
* Performance: tight inner loops (log for one file, rebuild a tree) are
  in-process; no fork per commit.
* `gix` is promising but, as of writing, is still flagged as not yet
  feature-complete for our use cases (refspec evaluation, merge, network
  push over SSH agent). We can revisit and migrate later — both APIs are
  Rust, so the blast radius is contained to `vcs_git2.rs`.

### Consequences

* Good: typed errors, no porcelain parsing, no `fork()` per commit.
* Good: pulls in `libgit2` (vendored via `libgit2-sys`) which is well-
  tested in many Rust projects.
* Bad: C dependency. Builds need a C toolchain and OpenSSL/zlib headers
  (or accept the vendored versions). On Android NDK builds this is
  non-trivial — addressed by ADR-0003: `git2` lives in `bypass-cli`, never
  in `bypass-core`, so Android targets don't pay this cost unless the
  Android frontend opts in later.
* Bad: larger compiled binary than a pure-Rust solution would produce.

### Confirmation

`git2` MUST NOT appear in `crates/bypass-core/Cargo.toml`. The
`VersionControl` trait in [`bypass-core::vcs`](../../crates/bypass-core/src/vcs.rs)
is the seam; the concrete `vcs_git2.rs` impl lives in `bypass-cli`. See
[ADR-0003](0003-workspace-split-core-cli.md).

If we later want to migrate to `gix`, only `bypass-cli`'s `vcs_git2.rs` and
its `Cargo.toml` change. This ADR would be superseded at that point.
