<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Record architecture decisions

* Status: accepted
* Date: 2026-05-21
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass` will ship multiple frontends (Linux CLI, Android, browser extensions)
on top of a shared core. The roadmap already names several decisions
(platform-delegated crypto, pass-compatible layout, git2-only versioning,
workspace split, license) whose rationale is at risk of being lost once the
roadmap checkboxes are ticked. We need a durable, append-only record so
contributors — including future-us — can see *why* a thing is the way it is
without having to spelunk through git history.

## Considered Options

* Inline rationale comments in `CLAUDE.md` and `doc/ROADMAP.md` (status quo).
* A single `doc/DECISIONS.md` chronological log.
* One file per decision under `doc/adr/`, following the
  [MADR](https://adr.github.io/madr/) 4.x template.

## Decision Outcome

Chosen option: **one MADR file per decision under `doc/adr/`**, because:

* Each decision gets a stable URL/path that can be linked from code comments,
  PR descriptions, and other ADRs.
* MADR's "Considered Options" and "Consequences" sections force us to record
  the trade-offs, not just the verdict.
* A `0000-record-architecture-decisions.md` meta-ADR establishes the process
  itself, so the convention is self-documenting.

### Conventions

* Filenames: `NNNN-kebab-title.md`, four-digit zero-padded sequence number.
* Numbers are allocated in commit order and **never reused**, even for
  rejected or superseded ADRs.
* `Status` is one of: `proposed`, `accepted`, `rejected`, `deprecated`,
  `superseded by [ADR-XXXX](XXXX-…​.md)`.
* Once an ADR is `accepted` and merged, edits are limited to status changes
  and adding cross-links. Reversing a decision means writing a new ADR that
  supersedes the old one — don't rewrite history.
* Every ADR file starts with the project's standard SPDX header
  (`<!-- SPDX-License-Identifier: GPL-3.0-or-later -->`).

### Consequences

* Good: contributors have a single place to learn the project's load-bearing
  decisions, with the *why* preserved next to the *what*.
* Good: PR reviewers can require an ADR for any change that touches a
  decision recorded here, raising the bar on architectural drift.
* Bad: small overhead for each significant decision (write + review an extra
  file). Mitigated by keeping ADRs short.
