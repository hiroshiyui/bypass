<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Foreign-format importers: hybrid in-tree + extension model

* Status: proposed; extension wire format superseded by [ADR-0029](0029-importer-extension-wire-format.md)
* Date: 2026-05-23
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0026](0026-export-import-for-backup-and-rotation.md) defines a
bypass-native bundle format (GPG-wrapped tar with a manifest) and the
`bypass restore` command that ingests it. That covers backup,
migration between bypass installs, and GPG key rotation — but it does
not help a user who today keeps their passwords in **Bitwarden, 1Password,
LastPass, KeePass(XC), Enpass, Dashlane, Apple Passwords**, or any of
the long tail of competing managers. Without a clear on-ramp for
those users, "switch to bypass" means "hand-script a converter
yourself," which is a poor adoption story for a project whose entire
value proposition is "you should own your password store."

The design question is twofold:

1. **CLI shape.** Do we ship `import-bitwarden`, `import-1password`,
   `import-lastpass`, … as siblings of `import`, or a single
   `bypass import --format=<name>` that dispatches by format?
2. **Where the parsers live.** Are they all in-tree (bypass owns
   every format's parser), all out-of-tree (each is a `bypass ext`
   extension — see [`extensions.rs`](../../crates/bypass-cli/src/extensions.rs)),
   or a mix?

Both questions interact with two things bypass already committed to:
the first-line-password-then-key-value entry shape from
[ADR-0002](0002-pass-compatible-on-disk-layout.md) /
[`entry.rs`](../../crates/bypass-core/src/entry.rs), and the
core/CLI split from [ADR-0003](0003-workspace-split-core-cli.md)
which forbids `bypass-core` from doing I/O or network work.

## Considered Options

* **A. All in-tree, separate subcommands per format.**
  `bypass import-bitwarden <file>`, `bypass import-1password <file>`,
  etc. Tightest UX, every format works out of the box, no extension
  install step.
* **B. All in-tree, single subcommand with `--format`.**
  `bypass import --format=bitwarden <file>`. Still tightest UX, but
  CLI surface stays bounded as formats are added.
* **C. All out-of-tree via `bypass ext`.** Each importer is a
  separate executable (e.g. `bypass-import-bitwarden`) discovered the
  same way the existing pass-extension mechanism discovers them. Each
  extension emits an [ADR-0026](0026-export-import-for-backup-and-rotation.md)
  bundle on stdout; `bypass import` (or a thin `bypass import
  --from-ext bitwarden <file>` wrapper) ingests it. Zero core-surface
  growth.
* **D. Hybrid: a small first-party set in-tree behind `--format=`,
  long tail via extensions emitting the bundle format.** First-party
  set is **Bitwarden, KeePass (KDBX-XML export), and generic
  RFC-4180 CSV** — the formats most users actually arrive from and
  the ones whose schemas are stable enough that we can support them
  long-term. Everything else (1Password, LastPass, Enpass, Dashlane,
  Apple Passwords, NordPass, Proton Pass, …) goes via the extension
  mechanism.

## Decision Outcome

Chosen option: **D — hybrid first-party + extension, dispatched by
`bypass import --format=<name> <file>`.**

The surface:

```text
# First-party, in-tree:
bypass import --format=bitwarden  vault.json
bypass import --format=keepass    export.xml
bypass import --format=csv        passwords.csv      [--csv-schema=…]

# Long-tail, via extension producing an ADR-0026 bundle internally:
bypass import --from-ext 1password export.1pux
```

Semantics and division of labour:

* **Parsing** of first-party formats lives in `bypass-core`
  (`bypass_core::import::<format>`). Pure logic: bytes in, an
  iterator of `ImportedEntry` out. No filesystem, no network, no
  subprocess — same discipline as the rest of `bypass-core`
  ([ADR-0003](0003-workspace-split-core-cli.md)).
* **Dispatch, file/stdin I/O, and the per-entry GPG encrypt + commit**
  live in `bypass-cli` (`import.rs`). The driver iterates the parser,
  hands each `ImportedEntry` through the same code path that
  [ADR-0026](0026-export-import-for-backup-and-rotation.md)'s
  `import` already uses to encrypt + write + auto-commit. **There is
  exactly one write path**; the format-specific code stops at "I
  produced `ImportedEntry` values."
* **Extensions** for the long tail are discovered the way the
  existing [`extensions.rs`](../../crates/bypass-cli/src/extensions.rs)
  mechanism already discovers `pass`-style extensions. Their
  contract is narrow: take a source-vault path on `argv`, write a
  valid [ADR-0026](0026-export-import-for-backup-and-rotation.md)
  bundle to stdout, exit non-zero on failure. The user never sees
  the bundle: `bypass import --from-ext <name> <file>` invokes
  `bypass-import-<name>` internally, captures its stdout, and feeds
  the bundle through the same reader that powers `bypass restore`.
  The bundle is an **internal IPC contract** between the extension
  and bypass — not a user-facing surface — which keeps the verb
  separation clean (`import` = foreign → bypass, `restore` =
  bypass-native bundle → bypass) and reuses the bundle format we
  *already* designed, so we don't ship a second IPC schema.

### Canonical mapping rules (`ImportedEntry` → bypass entry)

These are normative for every importer, in-tree or extension. Stated
explicitly here so behaviour is consistent across formats and so
extension authors have one place to read.

* **`ImportedEntry` shape**: `{ path: RelPath, password: SecretBytes,
  username: Option<String>, fields: Vec<(String, String)>, totp:
  Option<String>, notes: Option<String>, uris: Vec<String> }`.
* **Serialised entry layout** (matching
  [ADR-0002](0002-pass-compatible-on-disk-layout.md) and
  [`entry.rs`](../../crates/bypass-core/src/entry.rs)):
  1. First line: `password` verbatim.
  2. `login: <username>` if present.
  3. One `key: value` line per `fields` entry, key trimmed, value
     trimmed, newlines in values replaced with `\n` (Bitwarden's
     custom fields *do* allow embedded newlines; collapsing keeps
     the entry parseable).
  4. `otpauth: otpauth://totp/...` if TOTP present, in the form
     `bypass-core::otp` already parses.
  5. `url: <uri>` for the first URI; subsequent URIs as
     `url-2: …`, `url-3: …`, …. Matches what users coming from
     pass-style multi-URL conventions already expect.
  6. Free-form `notes` last, prefixed by a blank line, no key.
* **Path derivation**: `<folder-path>/<item-name>`. Folder path
  comes from the source's folder/group tree; item name is the
  source's display name. Both are slugified by:
  * Lowercasing.
  * Replacing whitespace runs with `-`.
  * Stripping characters outside `[a-z0-9._-/]`.
  * Collapsing repeated `/` and `-`.
* **Collision policy**: if the derived path collides with an
  existing entry *in the same import batch*, suffix `-2`, `-3`, …
  until unique. If it collides with an entry **already in the
  store**, fail the entire import with a list of conflicting paths;
  do not partial-apply. (Same atomicity rule
  [ADR-0026](0026-export-import-for-backup-and-rotation.md)
  established for `import`.)
* **Lossiness disclosure**: every importer **must** print a stderr
  summary at the end listing fields it dropped or transformed
  (embedded newlines collapsed, unsupported attachment counts, custom
  field types coerced to strings). Imports are one-shot operations;
  the user needs to know what didn't survive *before* they delete
  their source vault.

### Reasoning

* **Why hybrid, not all-in-tree.** The long tail is genuinely
  unbounded and each format's parser is a bug surface forever. Tying
  every new format to a bypass release would slow the project and
  push us to half-maintain parsers for managers we don't personally
  use. Letting the community ship importers as extensions matches the
  pass tradition and matches what bypass already does with `bypass
  ext` for non-import use cases.
* **Why not all-out-of-tree.** Bitwarden in particular is the
  single most-asked-about migration path for any pass-family
  manager, and its JSON export schema is stable and well-documented.
  Shipping it as an extension would mean every user installs an
  extension on day one, which is a worse onboarding story than `apt
  install bypass`. KeePass and CSV are similar: stable, common,
  worth owning.
* **Why `--format=` dispatch and not separate subcommands.** Adding
  `import-bitwarden`, `import-1password`, `import-lastpass`, …
  bloats `bypass --help`, pollutes shell completion, and makes
  `import` (bypass-native) feel like a separate concept from
  importing in general. With `--format=`, the same verb does the
  same job — encrypt entries into this store — and only the parser
  differs. Discoverability is preserved by `bypass import --help`
  listing supported formats and pointing at the extension protocol
  for the rest.
* **Why one write path.** The single biggest correctness risk in
  importer work is "two slightly different code paths both write
  entries, and one forgets to honour `.gpg-id` resolution / one
  doesn't auto-commit / one skips the leak audit." Funnelling every
  importer (in-tree *and* extension) through the same
  `ImportedEntry` → encrypt → commit path makes those mistakes
  unrepresentable.
* **Why the bundle format is the extension contract.** We already
  designed and tested a serialised "set of entries plus a manifest"
  representation for [ADR-0026](0026-export-import-for-backup-and-rotation.md);
  reusing it as the extension wire format means extension authors
  have one schema to target, and `bypass import` has one parser to
  maintain. The cost is one extra round-trip
  (parser→bundle→re-parse) per imported vault, which is irrelevant
  at one-shot-migration cadence.

### Consequences

* Good: bypass ships with a credible day-one migration story for the
  three most common source vaults (Bitwarden, KeePass, CSV) without
  committing to maintain parsers for the long tail.
* Good: extension authors get a precise, stable contract (emit an
  ADR-0026 bundle on stdout) and don't need to understand
  `bypass-core` internals or recipient resolution.
* Good: `--format=` dispatch keeps the CLI surface flat as new
  formats are added; man-page and completion costs are bounded.
* Bad: Bitwarden's **encrypted** export (`encrypted_json`) needs the
  user's Bitwarden master password to decrypt *before* our importer
  can read it. The importer must prompt for it, hold it in
  `SecretBytes`, never echo it, and ideally accept it on a dedicated
  fd rather than tty so scripted imports are possible. Same shape as
  any future "decrypt-then-import" format.
* Bad: the canonical mapping is *opinionated*. Users with rich
  custom-field schemas in 1Password or KeePass will lose structure
  on import (everything flattens to `key: value` lines). This is
  inherent to the bypass entry shape, not something the mapping rules
  can fix. The lossiness summary on stderr is the mitigation.
* Bad: an extension producing a malformed bundle is a foot-gun (the
  user might not notice until they `bypass show` an entry and get
  garbage). `bypass import` validates the bundle's manifest schema
  version (per [ADR-0026](0026-export-import-for-backup-and-rotation.md))
  and refuses on mismatch, but it cannot validate that the *content
  inside* the bundle reflects what was in the source vault.
* Neutral: `bypass-core::import` is the first place where the core
  crate grows format-specific code. We keep it isolated in a
  submodule per format (`import::bitwarden`, `import::keepass`,
  `import::csv`) so the rest of the crate stays format-agnostic.

### Confirmation

* Implementation lands as a new milestone in `doc/ROADMAP.md` after
  ADR-0026's Milestone 4.4 (the bundle format must exist first).
  Likely Milestone 4.5; the milestone's checkbox is the confirmation
  the design described here was executed.
* Tests live in `crates/bypass-cli/tests/end_to_end.rs`: a Bitwarden
  fixture (plain JSON) and a KeePass fixture (KDBX-XML) round-trip
  through `bypass import --format=…` into a throwaway store; each
  asserts entry paths, decrypted passwords, and a representative
  custom field. A separate test drives a tiny in-repo stub extension
  (under `tests/fixtures/`) to confirm the extension dispatch path
  works end-to-end.
* The extension contract (stdin/stdout shape, exit codes, bundle
  format version) is documented in a new `doc/extensions/importer-protocol.md`
  written as part of the milestone, not in this ADR — the ADR is the
  *why*, the protocol doc is the *how* and will evolve as we add
  formats.
