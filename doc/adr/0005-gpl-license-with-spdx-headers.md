<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# License `bypass` under GPL-3.0-or-later, with SPDX headers on every file

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

A password manager handles the user's most sensitive secrets. The license
we ship under shapes:

* Whether downstream redistributors can keep modifications private.
* Whether the user has a *practical* (not just legal) right to inspect,
  build, and audit the binary on their device.
* How easily reviewers and tooling (REUSE, SPDX scanners, OSS package
  policies) can attribute and verify the license of each source file.

`pass`, which `bypass` is a reimplementation of, is GPL-2.0-or-later.

## Considered Options

* **MIT or Apache-2.0**: permissive; maximises adoption; allows closed
  proprietary forks.
* **MPL-2.0**: weak copyleft per-file; modifications to MPL files must
  remain MPL; surrounding code can stay proprietary.
* **GPL-3.0-or-later**: strong copyleft over the program as a whole;
  modifications redistributed in binary form must come with corresponding
  source.

## Decision Outcome

Chosen option: **GPL-3.0-or-later**, with an SPDX header on every source
file.

* Aligns with `pass` (GPL-2.0-or-later) and with the broader Free Software
  password-manager ecosystem (KeePassXC: GPL-3.0; gopass: MIT but draws on
  pass; OpenKeychain: GPL-3.0). Anyone who builds *on top* of `bypass`
  stays in the same ecosystem.
* "or-later" lets future maintainers move to a successor licence (GPL-4 if
  it ever exists, or a fix-up release) without re-collecting consent from
  every contributor.
* For a security-sensitive tool, the strong "source available on
  redistribution" guarantee is a feature, not a tax: it raises the cost of
  shipping a tampered binary, since the recipient is legally entitled to
  the exact source.
* SPDX headers (`SPDX-License-Identifier: GPL-3.0-or-later`) on each file
  make licence detection trivial for tooling and unambiguous in code
  review, even when files are copy-pasted out of context.

### Consequences

* Good: contributors and downstream forks must share their changes;
  user freedom is preserved end-to-end.
* Good: SPDX headers are machine-readable; tools like
  [REUSE](https://reuse.software/) can verify compliance.
* Bad: some prospective users (corporate, embedded) may avoid GPL code.
  Acceptable — those users have plenty of permissively-licensed managers.
* Bad: discipline tax: every new source file (`*.rs`, build scripts,
  future shell/Kotlin/TS sources) must begin with the SPDX header. PRs
  must be reviewed for this. Tooling can backfill this if it becomes a
  papercut.

### Confirmation

* The repository root contains `LICENSE` (GPL-3.0-or-later text) and the
  workspace `Cargo.toml` sets `license = "GPL-3.0-or-later"`.
* Every new source file MUST begin with a comment-syntax-appropriate SPDX
  header (`//`, `#`, `<!--`, …). Vendored third-party code is exempt.
* Reviewers reject PRs that introduce files without the header.
