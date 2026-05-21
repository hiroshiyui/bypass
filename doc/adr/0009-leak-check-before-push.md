<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Refuse to push files that don't look like OpenPGP ciphertext

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass`'s worst possible failure mode is publishing a user's
plaintext secret to a git remote. Several ways this can happen:

1. An editor crash leaves `.work.gpg.swp` or `work.gpg~` in the
   store. The user runs `bypass git add . && bypass git push`
   (or the future `bypass sync`) without noticing.
2. A script wrongly writes a plaintext blob into the store
   directory using the `.gpg` extension (e.g. testing).
3. A misconfigured `bypass edit` writes the editor's autosave file
   alongside the encrypted entry.
4. The user manually copies a `notes.txt` containing secrets into
   the store and commits it.

`pass` itself has no defence against any of this. Once published,
the plaintext is on the remote forever (and in every clone's
reflog). Recovering means rotating every secret in the affected
commit — for many users that's their entire store.

This ADR records the decision to ship a defence-in-depth backstop:
before pushing, inspect every file in the unpushed commits and
refuse the push when anything looks like plaintext.

## Considered Options

* **Do nothing.** Match `pass`. Cheapest; relies entirely on the
  user being careful.
* **Warn, but proceed.** Surface a stderr warning when something
  looks off, then push anyway. Falls foul of "users don't read
  stderr"; defeats the security goal.
* **Refuse-by-default with `--force` override.** Inspect each file
  about to be pushed; fail the push when any file isn't a `.gpg`
  with an OpenPGP packet header, a known metadata file
  (`.gpg-id`, `.gitignore`, …), and isn't an editor-backup pattern.
  Provide an audit subcommand for diagnostic use and an explicit
  override flag for edge cases.

## Decision Outcome

Chosen option: **refuse-by-default with `--force` override**.

Implementation:

- A new `bypass-cli::audit` module provides `check_file(path,
  head)`, `check_files(iter)`, and `audit_for_push(store_root)`.
- The OpenPGP header sniff is a byte-pattern check (RFC 4880 §4.2):
  first byte `0x80..=0xFF` (binary new- or old-format header) **or**
  the ASCII-armour prefix `-----BEGIN PGP MESSAGE-----`. We
  deliberately don't parse the packet — a valid header rules out
  plaintext, which is the entire point of this check.
- Filename allowlist for non-`.gpg` files: `.gpg-id`,
  `.gpg-id.sig`, `.gitignore`, `.gitattributes`, `README*`,
  `LICENSE*`.
- Editor-backup detection: `*~`, `.*.swp`, `.*.swo`, `*.orig`,
  `*.rej`, `*.bak`, `#*#`.
- Scope: files in unpushed commits, i.e.
  `git diff --name-only @{upstream}..HEAD`. Falls back to
  `git ls-files` when no upstream is configured (initial sync).
- `bypass sync` runs the audit between `git pull --rebase` and
  `git push`; on any issue, it lists the offenders and exits
  non-zero. `bypass sync --force` skips the check.
- `bypass audit` exposes the same check as a standalone diagnostic
  subcommand. Exit 0 = clean, exit 1 = issues found.
- `bypass doctor` adds an `audit` row that runs the same check
  against post-init stores.

### Consequences

* Good: the worst-case `bypass` mistake — plaintext escaping to the
  remote — is now caught at the last possible moment before the
  network call. The same logic protects every channel
  (`bypass sync`, `bypass audit`, `bypass doctor`).
* Good: no new dependencies. The OpenPGP header sniff is two
  byte-pattern checks; we don't pull in a libpgp-shaped crate
  just to validate a header.
* Good: the check is *additive*. `bypass sync --force` and direct
  `bypass git push` both bypass it cleanly, so users with weird
  workflows are not blocked — they just have to opt out
  explicitly.
* Bad: false positives are possible on stores users hand-curate
  with their own filenames (e.g. a deliberately committed
  `passwords.json.example`). The escape hatches (`--force`,
  contributing to the allowlist, or going through `bypass git
  push` directly) keep this from being a hard wall.
* Bad: the gap covers only `bypass sync` — a user who runs
  `bypass git push` directly bypasses the audit. Closing that gap
  via a pre-push git hook installed by `bypass init` is on the
  roadmap-but-not-yet (see [out-of-scope in the milestone plan]).
* Bad: the header sniff matches anything with a high bit set in
  the first byte. A user maliciously crafting a "plaintext" file
  that begins with such a byte would pass. Accepted: this is a
  social-engineering scenario, not an accidental-leak one, and
  the threat model here is accidental leaks.

### Confirmation

* Implementation: [`crates/bypass-cli/src/audit.rs`](../../crates/bypass-cli/src/audit.rs).
* Unit tests: 9 in `audit::tests` covering each `LeakKind`, the
  packet-header positive paths (binary new/old format and ASCII
  armour), the empty-`.gpg` case, the editor-backup patterns, and
  the allowlist.
* Integration tests in
  [`crates/bypass-cli/tests/end_to_end.rs`](../../crates/bypass-cli/tests/end_to_end.rs):
  `sync_refuses_when_plaintext_is_staged` (refusal + remote ref
  unchanged + `--force` override advances it) and
  `audit_lists_problem_files` (standalone subcommand surfaces the
  same issues).
* `bypass doctor` row aggregates the same check into the post-init
  smoke report.

### Related

* [ADR-0001](0001-platform-delegated-crypto.md) — `bypass-core`
  doesn't speak OpenPGP, but the audit's byte-pattern sniff is
  format-aware enough to recognise gpg's output without parsing
  packets.
* [ADR-0008](0008-secure-delete-via-overwrite.md) — same
  "defence-in-depth at the boundary" philosophy: tighten the worst
  case (`rm` leaving recoverable plaintext, `push` publishing
  plaintext) without forcing every backend to participate.
