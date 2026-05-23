<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Importer-extension wire format: newline-delimited JSON over stdout

* Status: accepted; amends [ADR-0027](0027-foreign-format-importers.md) (extension wire format)
* Date: 2026-05-23
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0027](0027-foreign-format-importers.md) commits to a hybrid
in-tree + extension model for foreign-vault importers: first-party
Bitwarden / KeePass / CSV parsers ship in `bypass-core`, the long
tail (1Password, LastPass, Enpass, Dashlane, …) lives in
`bypass-import-<name>` extensions discovered via the existing
[`extensions.rs`](../../crates/bypass-cli/src/extensions.rs)
mechanism.

ADR-0027's "Decision Outcome" section specifies the **wire format**
between the extension and `bypass import --from-ext`:

> Extension authors get a precise, stable contract (emit an ADR-0026
> bundle on stdout) and don't need to understand bypass-core
> internals or recipient resolution.

Implementing it surfaced the cost of that choice. An ADR-0026
bundle is:

1. A ustar archive (extension needs a tar writer).
2. With per-entry plaintexts at **already-slugged** paths under
   `entries/<RelPath>` — meaning the extension needs to implement
   the [ADR-0027 canonical mapping](0027-foreign-format-importers.md)
   (`slug_path`, in-batch collision suffixing, the
   `password\nlogin:\nkey: value\n…\nurl:\n\nnotes` body shape).
3. With a `manifest.toml`.
4. Wrapped in a GPG ciphertext keyed to the destination's `.gpg-id`
   recipients (the extension needs to spawn `gpg --encrypt` and
   know which recipients to use; `bypass` would have to pass the
   recipients as argv).

For a Python / shell extension that just wants to convert a
proprietary export to a sequence of `{name, password, ...}` records,
all four layers are pure overhead. They also push the canonical
mapping out of one place (`bypass-core::import::prepare`, which we
already test) into every extension — silently divergent slugging
across the long tail is a bug-class we'd own forever.

The plaintext-confidentiality argument that justified GPG-wrapping
for `bypass backup` (the bundle lives on disk) does not apply here:
the extension's stdout is an **OS pipe**, in-process, never touches
disk.

## Considered Options

* **A. Keep ADR-0027's bundle wire format.** Status quo. Every
  extension author re-implements slugging + body serialisation +
  tar + GPG-encrypt. We own the cross-extension consistency risk.
* **B. Newline-delimited JSON of raw `ImportedEntry` records on
  stdout.** Extension writes one JSON object per line, each
  carrying the parser-output shape we already use in-tree
  ([`bypass_core::import::ImportedEntry`](../../crates/bypass-core/src/import.rs)).
  `bypass` reads the stream, runs the *same* canonical mapping +
  collision handling + write path the in-tree parsers use. No
  outer encryption (pipe is in-process); no tar packing; no
  recipient knowledge in the extension.
* **C. Some other framing (length-prefixed binary, BSON, ...).**
  Smaller wire size, but for one-shot vault imports (typically
  <10 MB of plaintext, hundreds of entries) the savings don't
  justify the complexity. JSON over a line stream is what every
  shell-friendly CLI already speaks.

## Decision Outcome

Chosen option: **B — newline-delimited JSON of raw `ImportedEntry`
records on stdout**, amending ADR-0027's "Decision Outcome" section
on the extension wire format.

### The contract

`bypass-import-<name>` is invoked with:

```text
bypass-import-<name> <source-file>
    PASSWORD_STORE_DIR=<store-root>
    PASSWORD_STORE_BIN=<absolute path to the running bypass binary>
```

(Same `argv` + env shape as any `bypass ext` extension — see
[`extensions.rs`](../../crates/bypass-cli/src/extensions.rs).)

The extension writes **one JSON object per line** to stdout. Each
line is a single record with this schema:

```json
{
  "folder": ["Personal", "Email"],
  "name": "GitHub",
  "password": "hunter2",
  "username": "alice",
  "fields": [["recovery", "kitten"], ["pin", "1234"]],
  "totp": "otpauth://totp/x?secret=ABC",
  "notes": "free-form text, can contain real newlines",
  "uris": ["https://github.com", "https://github.com/mobile"]
}
```

Required: `name`, `password` (may be the empty string for secure-
note-style records, but the field must be present).

Optional, all default to absent/empty: `folder`, `username`,
`fields`, `totp`, `notes`, `uris`.

The extension exits `0` on success; any non-zero exit fails the
import. Anything written to stderr is forwarded verbatim to the
user's terminal (typical use: prompts, progress, warnings).

### What `bypass` does on its side

1. Spawn the extension with `Stdio::piped()` for stdout, stderr
   inherited.
2. Read stdout line-by-line; parse each as one `ImportedEntry`.
   Malformed lines are surfaced as a clean error naming the byte
   offset; partial imports are atomic per [ADR-0027](0027-foreign-format-importers.md).
3. Wait for the child; non-zero exit fails the import.
4. Run the parsed records through the *same*
   [`bypass_core::import::prepare`](../../crates/bypass-core/src/import.rs)
   that powers `--format=bitwarden|csv|keepass`: slugging, in-
   batch collision suffixing, store-collision atomic-fail, body
   serialisation.
5. Encrypt + write each entry via `Store::insert_no_commit`, then
   commit the whole batch under `bypass: Import N entries from
   <name>` (matches the in-tree importer commit shape).

### Reasoning

* **One canonical mapping, owned in one place.** Every importer —
  in-tree and extension — goes through `import::prepare`. Slugging
  bugs get fixed once. The "diverging slugging across the long
  tail" failure mode is structurally impossible.
* **Extensions can be written in any language with a JSON
  serialiser** (i.e. every language). The 1Password `.1pux`
  extension reduces to: read the zip, iterate items, print JSON.
  No tar writer, no gpg invocation, no recipient handling.
* **Plaintext is not less safe on a pipe than in an ADR-0026
  bundle.** The bundle was wrapped because it lives on disk
  ([ADR-0026](0026-export-import-for-backup-and-rotation.md)
  rationale). An OS pipe is bounded by process lifetime; the
  parent process drains it as it arrives and feeds individual
  records straight into `SecretBytes`. The plaintext never lives
  longer than one entry's worth at a time, same as the in-tree
  parsers.
* **Streaming-friendly.** NDJSON is the Unix shell's natural
  streaming format. A future "import 100k records" use case
  doesn't need a different protocol — just keep printing lines.
* **No new dependencies.** `serde_json` already lives in
  `bypass-core` (added in Milestone 4.5.b for the Bitwarden
  parser).

### What ADR-0027 still gets right

The hybrid in-tree + extension *model* and the `bypass import
--format=<name>` / `bypass import --from-ext <name>` CLI surface
are unchanged. Only ADR-0027's "extension wire format" subsection
is amended.

## Consequences

### Good

* Extension authors target one tiny schema. The 1Password example
  in [`doc/extensions/importer-protocol.md`](../extensions/importer-protocol.md)
  is ~30 lines of Python; the equivalent ADR-0026-bundle extension
  would be 150+ lines.
* Slugging, collision policy, and entry-body shape stay testable
  and changeable in `bypass-core` without breaking every
  extension.
* No `gpg` invocations inside extensions; nothing for an extension
  bug to leak.
* Bundle format ([ADR-0026](0026-export-import-for-backup-and-rotation.md))
  stays single-purpose (`bypass backup` / `bypass restore`); we
  don't conflate "backup snapshot" with "importer IPC".

### Bad

* Two distinct wire shapes in the codebase: the ADR-0026 bundle
  (for `backup`/`restore`) and NDJSON `ImportedEntry` (for
  `--from-ext`). Acceptable — they serve different use cases.
* Plaintext flows over a pipe rather than through an encrypted
  envelope. As discussed, the lifetime is no worse than the in-
  tree parsers' (`SecretBytes` from the moment the record is
  decoded). Documenting this in the protocol doc is enough.
* Records crossing an OS pipe are JSON-escaped UTF-8, so a
  password containing exactly the bytes of a non-UTF-8 sequence
  would have to be base64-encoded by the extension into a
  custom field. The dominant case (UTF-8 passwords) is
  unaffected; the corner case can be addressed later with a
  `password_base64` companion field if a real user needs it.

## Confirmation

* Implementation: `bypass import --from-ext <name> <source-file>`
  in [`crates/bypass-cli/src/import.rs`](../../crates/bypass-cli/src/import.rs).
* Protocol documentation: [`doc/extensions/importer-protocol.md`](../extensions/importer-protocol.md)
  (the *how*, evolving as we add formats; this ADR is the *why*).
* Tests: a stub `bypass-import-stub` shell script under
  `crates/bypass-cli/tests/fixtures/` round-trips through
  `bypass import --from-ext stub <source>` in `end_to_end.rs`.

## Related ADRs

* [ADR-0027](0027-foreign-format-importers.md): the parent. This
  amendment narrows its "extension wire format" subsection while
  preserving every other decision (hybrid model, single `import
  --format=` verb, canonical mapping rules, atomic-fail collision
  policy, mandatory lossiness summary).
* [ADR-0026](0026-export-import-for-backup-and-rotation.md): the
  bundle format remains the one true wire shape for
  `backup`/`restore`, no longer doing double duty.
