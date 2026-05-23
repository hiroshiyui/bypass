<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# `bypass-import-<name>` extension protocol

This document describes the **wire contract** between a
`bypass-import-<name>` extension and `bypass import --from-ext
<name>`. It is the *how* — the design rationale lives in
[ADR-0027](../adr/0027-foreign-format-importers.md) and
[ADR-0029](../adr/0029-importer-extension-wire-format.md).

## When to write an extension

`bypass` ships first-party importers for **Bitwarden** plain-JSON,
**KeePass / KeePassXC** XML, and **generic CSV**. Anything else
goes via an extension — 1Password's `.1pux`, LastPass CSV
(with non-standard quoting), Enpass, Dashlane, NordPass, Proton
Pass, Apple Passwords, and so on.

If the format you target is reasonably common and stable, consider
upstreaming it as a first-party parser instead. The bar is roughly:
"I've personally used this manager and want to migrate off it."

## Where the extension lives

Discovery order — first match wins, same as `bypass ext` (see
[`extensions.rs`](../../crates/bypass-cli/src/extensions.rs)):

1. `<store-root>/.extensions/bypass-import-<name>` — store-local,
   travels with the git repo.
2. `$PASSWORD_STORE_EXTENSIONS_DIR/bypass-import-<name>` — if the
   env var is set.
3. `~/.password-store-extensions/bypass-import-<name>` — the
   conventional user-level directory.

The file must be a regular file with at least one execute bit set
(`chmod +x` it).

The `<name>` is whatever the user types after `--from-ext`. Pick
something short and clearly tied to the source manager:
`bypass-import-1password`, `bypass-import-lastpass`, etc.

## Invocation

`bypass` invokes the extension with **one positional argument**
(the source file path) and **two environment variables**:

```text
bypass-import-<name> <source-file>
    PASSWORD_STORE_DIR=<absolute path to the destination store>
    PASSWORD_STORE_BIN=<absolute path to the running bypass binary>
```

- `<source-file>` is whatever the user passed as the import source.
  The extension may treat it as a file path, a directory path
  (1Password's `.1pux` is a zip), or any other URL-like string —
  `bypass` does not look at it itself.
- `PASSWORD_STORE_DIR` lets the extension know which store the
  user is importing into, in case it matters (it usually doesn't —
  recipient resolution is `bypass`'s job).
- `PASSWORD_STORE_BIN` is provided for symmetry with other pass-
  style extensions; an importer rarely needs to call back into
  `bypass`.

## Wire format

The extension writes **one JSON object per line** to stdout. Each
line is one entry's pre-mapping record (in `bypass-core` terms:
one [`ImportedEntry`](../../crates/bypass-core/src/import.rs)).
`bypass` runs every record through its canonical mapping — slugging,
in-batch collision suffixing, body serialisation — so the
extension *must not* slug paths or serialise entry bodies itself.

### Schema

```json
{
  "folder":    ["Personal", "Email"],
  "name":      "GitHub",
  "password":  "hunter2",
  "username":  "alice",
  "fields":    [["recovery", "kitten"], ["pin", "1234"]],
  "totp":      "otpauth://totp/x?secret=ABC",
  "notes":     "free-form text, can contain real newlines",
  "uris":      ["https://github.com", "https://github.com/mobile"]
}
```

| Field      | Type                       | Required | Notes                                                                                                 |
| ---------- | -------------------------- | -------- | ----------------------------------------------------------------------------------------------------- |
| `name`     | string                     | yes      | Source-vault display name. May contain `/` — multi-segment names like `"Email/Work"` are honoured.    |
| `password` | string                     | yes      | UTF-8 password. May be the empty string for note-style records.                                       |
| `folder`   | array of strings           | no       | Folder/group path, source order. Each element is one segment; *do not* embed `/` here — use the array.|
| `username` | string                     | no       | Becomes `login: <value>` in the entry body. Omit or pass `""` to skip.                                |
| `fields`   | array of `[string, string]`| no       | Extra `key: value` lines, in source order. Empty keys are dropped.                                    |
| `totp`     | string                     | no       | Pre-formatted `otpauth://totp/...` URI (the same shape KeePassXC's `otp` field uses).                 |
| `notes`    | string                     | no       | Free-form. Real newlines are preserved.                                                               |
| `uris`     | array of strings           | no       | First becomes `url:`; subsequent entries become `url-2:`, `url-3:`, …                                 |

Unknown JSON keys are silently ignored (the schema is forward-
extensible).

### Framing

- Each record is **exactly one line** of JSON. No pretty-printing
  inside a record.
- Blank lines (between records, leading, trailing) are tolerated
  as separators.
- The stream ends at EOF. No explicit terminator.

### Stderr

Anything the extension writes to stderr is forwarded verbatim to
the user. Use it for progress, prompts, or warnings:

```text
extension: reading vault.1pux (40 items)
extension: prompting for master password... [reads from /dev/tty]
extension: imported 40 of 40
```

`bypass` itself prints a final lossiness summary on stderr at the
end of the import, naming everything it had to slug/disambiguate.

### Exit code

- `0` — success. `bypass` processes every record received.
- non-zero — failure. `bypass` aborts the import atomically: no
  entries are written. Whatever the extension wrote on stderr is
  the user's primary diagnostic; `bypass` adds its own one-line
  context.

## Worked example — Python skeleton

```python
#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
#
# bypass-import-1password — minimal sketch (NOT a complete 1pux
# parser). Reads the source file path from argv[1] and emits one
# JSON record per line on stdout.

import json
import sys
import zipfile

def main() -> int:
    if len(sys.argv) < 2:
        print("usage: bypass-import-1password <source.1pux>", file=sys.stderr)
        return 2
    src = sys.argv[1]

    with zipfile.ZipFile(src) as z:
        # ... real implementation: walk z's items, decrypt fields,
        # convert to the record shape below ...
        records = [
            {
                "folder": ["personal", "email"],
                "name": "GitHub",
                "password": "hunter2",
                "username": "alice",
                "uris": ["https://github.com"],
            }
        ]

    for r in records:
        sys.stdout.write(json.dumps(r, ensure_ascii=False))
        sys.stdout.write("\n")
    return 0

if __name__ == "__main__":
    sys.exit(main())
```

Install:

```sh
install -m 0755 bypass-import-1password ~/.password-store-extensions/
bypass import --from-ext 1password ~/Downloads/vault.1pux
```

## Worked example — shell skeleton

For trivially-shaped exports (the test fixture is shaped like this):

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

# argv[1] is the source file; PASSWORD_STORE_DIR / PASSWORD_STORE_BIN
# are also exported by `bypass`. Ignore what isn't needed.
src="${1:?source file required}"

# Convert the source to NDJSON. Here we just emit two static
# records; a real implementation would parse $src.
cat <<'JSON'
{"folder":["personal","email"],"name":"Gmail","password":"hunter2","username":"alice"}
{"folder":["work"],"name":"Office365","password":"p2","fields":[["pin","1234"]]}
JSON
```

## Security notes

- **Plaintext crosses an OS pipe.** It is held in process memory
  only for the lifetime of the record it belongs to (`bypass`
  decodes one line into a `SecretBytes`, runs it through encrypt-
  and-commit, drops it). Same exposure window as the in-tree
  parsers.
- **No GPG inside the extension.** Recipient resolution is
  `bypass`'s job; the extension is format-agnostic about how the
  data gets encrypted at rest.
- **Don't `print()` debug dumps of the records.** Anything on
  stdout becomes a parse attempt; anything on stderr is shown to
  the user. Use stderr judiciously, and never echo plaintext
  passwords there even as a "test print".
- **If the source format itself is encrypted** (1Password's
  encrypted-export variant, Bitwarden's `encrypted_export.json`,
  KeePass `.kdbx` binary), the extension is responsible for
  prompting the user for the master password and decrypting in-
  process. Read the passphrase from `/dev/tty` (so a piped
  invocation can't supply one accidentally) and hold it in a
  cleared buffer.

## What `bypass` does on its side

For reference:

1. Locates `bypass-import-<name>` via the discovery order above.
2. Spawns it with the documented argv + env; captures stdout to a
   pipe, leaves stderr inherited.
3. Reads stdout line-by-line; decodes each line as one record.
4. Waits for the child; non-zero exit aborts.
5. Runs every record through `bypass_core::import::prepare`
   (slugging, collision handling, body serialisation).
6. Encrypts + writes each entry via `Store::insert_no_commit`,
   then commits the whole batch under
   `bypass: Import N entries from ext:<name>`.
7. Prints the mandatory lossiness summary on stderr.
