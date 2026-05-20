<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# `RelPath` newtype with strict traversal-safety invariants

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

Every blob inside a `bypass` store is addressed by a path. That path comes
from several sources we do not fully trust:

* User input on the CLI (`bypass show foo/../../../etc/passwd`).
* JSON requests from the browser extension over native messaging.
* Entries already on disk listed by the [`Storage`](../../crates/bypass-core/src/storage.rs)
  trait.

Frontends turn a path into a real filesystem path by joining it onto the
store root. If a caller can sneak in `..` or an absolute path, they can
read or overwrite arbitrary files. If they can sneak in a NUL, they can
truncate C-level path arguments inside `libgit2` or libc.

We need one place where this validation happens, so every backend gets it
for free.

## Considered Options

* **Validate ad-hoc** in each frontend's `Storage` impl.
* **Accept `std::path::PathBuf`** at the trait boundary and rely on each
  impl to call `canonicalize` and check the prefix.
* **A `RelPath` newtype** in `bypass-core` whose constructor enforces the
  invariants once; every trait method takes `&RelPath`.

## Decision Outcome

Chosen option: **`RelPath` newtype**, defined in
[`bypass-core::path`](../../crates/bypass-core/src/path.rs).

Invariants enforced at construction (`RelPath::new`):

* non-empty,
* no leading or trailing `/`,
* no empty segments (`//`),
* no `.` or `..` segments,
* no NUL bytes,
* no backslashes (POSIX-style separators only, regardless of host).

Frontends accept `&RelPath`; by the time their code runs, the value is
already known-good.

* Validation lives in exactly one place. Future backends (UniFFI, native-
  messaging host) inherit the guarantees for free.
* The invariants are *constructive*: a `RelPath` value is impossible to
  build in an unsafe state. Bugs that would otherwise surface as TOCTOU in
  the storage impl turn into compile-time type errors at the trait
  boundary.
* `PathBuf` is the wrong type at the trait boundary: it conflates
  separators, allows absolute paths, and carries platform-specific
  semantics (drive letters, UNC on Windows) that have no meaning inside a
  password store.

### Consequences

* Good: a single tested module (`path.rs`, with 11 unit tests) is the
  basis of the project's traversal-safety story.
* Good: the type signature of `Storage::read`/`write` documents to readers
  that ad-hoc string paths are not accepted.
* Bad: callers must explicitly construct `RelPath` from user input and
  handle the `Result`. Mild ergonomic tax. Acceptable.
* Bad: filenames containing `\` or beginning with a `.` segment (a
  legitimate Unix concept, e.g. hidden files) cannot be addressed. Pass
  itself does not use such names for entries; we accept the limitation.

### Confirmation

[`crates/bypass-core/src/path.rs`](../../crates/bypass-core/src/path.rs)
contains the implementation and tests. Any new traversal-safety rule
(e.g. forbidding NTFS reserved names) belongs in `RelPath::validate` and
this ADR, not scattered through `Storage` impls.
