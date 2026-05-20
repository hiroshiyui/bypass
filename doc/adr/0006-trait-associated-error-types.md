<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Use associated `Error` types on core traits, not a unified core error

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass-core` defines three trait seams — [`Crypto`](../../crates/bypass-core/src/crypto.rs),
[`Storage`](../../crates/bypass-core/src/storage.rs),
[`VersionControl`](../../crates/bypass-core/src/vcs.rs) — that frontends
implement (ADR-0001, ADR-0003). Each impl produces failures specific to
its backend: `gpg` exit codes and stderr; filesystem I/O; libgit2 status
codes. We need to decide how those errors surface through the trait.

## Considered Options

* **Concrete unified `core::Error` enum** with `Crypto(String)`,
  `Storage(io::Error)`, `Vcs(String)` variants — the shape the first cut
  of `error.rs` had.
* **Boxed trait object** — every method returns `Result<T, Box<dyn
  std::error::Error + Send + Sync>>`.
* **Associated `Error` type per trait**, with a `std::error::Error + Send +
  Sync + 'static` bound; the orchestrator wraps them into its own error
  enum.

## Decision Outcome

Chosen option: **associated `Error` type per trait**.

* The `gpg` provider can ship a rich error type that captures exit status
  and a `Vec<u8>` of stderr; the libgit2 provider can carry a `git2::Error`
  with its native status code; neither has to flatten into a string at the
  trait boundary. Callers that need the detail (`bypass-cli` formatting
  user-facing errors) can downcast or pattern-match on the concrete type.
* `bypass-core` does not need to import `io::Error`, `git2::Error`, or any
  platform-specific error type. The portability rule from ADR-0003 stays
  intact.
* A unified enum would force every backend to lossily stringify or commit
  to a single error shape. Boxing throws away type information entirely
  and makes downcasting the *only* recovery option.
* The bound `std::error::Error + Send + Sync + 'static` is what `anyhow`,
  `thiserror::Error`'s `#[from]`, and the trait-object boxing in
  `Box<dyn Error>` all require — picking it keeps integration painless.

### Consequences

* Good: backend error detail is preserved end-to-end; UI layers can render
  it richly.
* Good: `bypass-core::error::Error` stays small — it only enumerates
  failures the core itself produces (invalid `RelPath`, not-found,
  malformed entry, missing `.gpg-id`).
* Bad: `Store<C, S, V>` ends up generic with three type parameters and the
  orchestrator's error type must wrap three associated errors. Verbose,
  but mechanical; `thiserror` makes it readable.
* Bad: trait objects (`dyn Crypto`) get clunkier — methods that return a
  GAT-style associated `Result` cannot be made dyn-compatible without
  erasing the error to `Box<dyn Error>`. Accepted: we expect monomorphised
  use in the CLI and UniFFI-generated bindings on Android; nothing in the
  current plan needs `dyn Crypto`.

### Confirmation

The traits in `crates/bypass-core/src/{crypto,storage,vcs}.rs` declare
`type Error: std::error::Error + Send + Sync + 'static;`. The shared core
[`error::Error`](../../crates/bypass-core/src/error.rs) deliberately does
not list `Crypto`/`Storage`/`Vcs` variants; that wrapping is the
orchestrator's job.
