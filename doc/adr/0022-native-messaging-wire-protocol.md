<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Native-messaging wire protocol between `bypass` and the browser extension

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[Phase 7](../ROADMAP.md#phase-7--browser-extension-firefox--chrome)
ships a Firefox + Chrome browser extension that delegates every
crypto-sensitive operation back to the desktop `bypass` binary
via the browser's native-messaging protocol. The browser side
gets a thin UI (popup, search, copy); the existing
`bypass-core::Store` + `crypto_gpg::GpgCli` path stays
unchanged, holding GPG keys and the on-disk store the same way
they always have.

Two design questions for this ADR:

1. **What's the on-the-wire format** between the host
   (`bypass messaging-host` subprocess) and the extension?
2. **What ops does the extension actually need**, and how do
   errors propagate?

The framing is partly forced on us — Chrome and Firefox spawn
the native host process and stream length-prefixed JSON in
both directions, both ways using a `4-byte little-endian
length + UTF-8 JSON` envelope. Inside that envelope, the
schema is ours to pick.

## Considered Options

**Wire format inside the envelope:**

* **Newline-delimited JSON with a request-correlation `id`
  field.** Matches the
  [ADR-0018 sync-status protocol](0018-daemon-status-protocol.md)
  shape; trivial to debug with `socat` / `nc`-equivalent
  tooling; the same `serde_json` we already depend on parses
  it. Browser quotas (≤1 MB reply on Chrome) are honoured
  with explicit byte-count checks before write.
* CBOR / postcard. Smaller, denser; but the whole point of an
  exposed protocol is debuggability, and one host process per
  popup-open means we're not in a hot loop where the size
  delta matters.
* gRPC / Protobuf / Cap'n Proto. Wildly over-engineered for a
  single-host single-binary local pipe.

**Request envelope shape:**

* **`{"id": <int>, "op": "<name>", ...op-specific fields}`.**
  Symmetric with the response (also tagged by `id`), so the
  extension can pipeline if it ever needs to. The `op` string
  is the dispatch tag; clap-style sub-fields per op carry the
  args.
* Untagged JSON with op inferred from field presence. Brittle;
  one typo in the extension produces a confusing dispatch.

**Op surface:**

* **The seven ops the extension MVP needs**: `ls`, `find`,
  `show`, `insert`, `generate`, `otp`, `rm`. Each is a thin
  wrapper around the matching CLI subcommand — `Show` returns
  plaintext (or one field), `Generate` returns the new
  password, etc. The whole CLI surface is bigger; we surface
  the slice that's safe-to-trigger-from-a-browser-context.
* All CLI ops. Including `init`, `git`, `edit`, `cp`, `mv`,
  `audit`, `sync`. Each one widens the attack surface (a
  compromised extension could `init` over a real store; could
  `edit` to harvest plaintext; could `sync --force` to
  exfiltrate). Defer until a real UX justifies the addition.
* Just `ls` + `find` + `show`. Read-only surface. Tempting,
  but `insert` and `generate` are core password-manager UX —
  the user expects to add new entries from the browser. The
  threat reduction from omitting them isn't large.

**Error encoding:**

* **`{"id": <int>, "ok": false, "error": "<string>"}`.** One
  tag per reply. Every internal `Result::Err` gets mapped to
  this shape via a `to_user_error` helper that strips backtrace
  / internal-type detail (the extension never sees a Rust
  panic message verbatim).
* Throw / catch via exceptions. Native messaging has no such
  semantics; pretending it does means inventing protocol that
  doesn't match the underlying channel.

**Size cap on the wire:**

* **Replies cap at 512 KB.** Chrome and Firefox both document
  a 1 MB reply limit; we leave 512 KB of headroom for envelope
  overhead and avoid the browser silently truncating us on a
  huge entry. Plaintext that would exceed the cap returns
  `ok: false` with `error: "reply too large"` — the user can
  fall back to the CLI for that entry.
* No cap. Trusts the browser to handle a 1 MB-ish wire frame
  cleanly; doesn't actually deliver because the browser will
  drop the message above its undocumented per-implementation
  threshold.

## Decision Outcome

- **Framing:** 4-byte little-endian length prefix + UTF-8
  JSON, both directions. Forced by the browsers; not a choice.
  The host's framing reader/writer live in
  `crates/bypass-cli/src/messaging_host.rs`.

- **Request envelope** (serde-tagged on `op`):
  ```json
  {"id": 1, "op": "show", "path": "email/work"}
  {"id": 2, "op": "show", "path": "email/work", "field": "login"}
  {"id": 3, "op": "ls", "subpath": "email"}
  {"id": 4, "op": "find", "pattern": "github"}
  {"id": 5, "op": "insert", "path": "x", "plaintext": "...", "overwrite": false}
  {"id": 6, "op": "generate", "path": "x", "length": 20, "symbols": true, "in_place": false}
  {"id": 7, "op": "otp", "path": "x"}
  {"id": 8, "op": "rm", "path": "x", "recursive": false}
  ```

- **Response envelope**:
  ```json
  // Success:
  {"id": 1, "ok": true, "plaintext": "hunter2\nlogin: alice"}
  {"id": 2, "ok": true, "value": "alice"}
  {"id": 3, "ok": true, "entries": ["email/personal", "email/work"]}
  {"id": 4, "ok": true, "entries": ["github.com/you"]}
  {"id": 5, "ok": true}
  {"id": 6, "ok": true, "password": "..."}
  {"id": 7, "ok": true, "code": "123456"}
  {"id": 8, "ok": true}
  // Failure (every op uses the same shape):
  {"id": <int>, "ok": false, "error": "<message>"}
  ```

- **Op surface (v1):** the seven listed above. `init` / `git`
  / `edit` / `cp` / `mv` / `audit` / `sync` / `sync-daemon` /
  `messaging-host` are **not** dispatched — the host returns
  `ok: false, error: "unknown op"` for any other tag. New ops
  require a follow-up ADR.

- **Error encoding:** every error path uses `{id, ok: false,
  error}` with a sanitised string. The Rust side has a
  `to_user_error` helper that strips internal types and
  backtrace frames; panics in the dispatch loop terminate the
  host (the browser sees pipe-closed, the extension UI
  surfaces "host crashed").

- **Reply size cap:** 512 KB per reply. The host checks
  `bytes.len()` before writing and returns
  `ok: false, error: "reply too large (<n> bytes; max
  <cap>)"` instead. Documented in the host's docstring.

- **Concurrency:** the host processes one request at a time
  (Rust `Store` is synchronous). The extension can pipeline
  by sending multiple requests with distinct `id`s; replies
  may arrive out of order conceptually but in practice come
  back in receive order.

- **Plaintext handling on the host:** every reply path that
  carries decrypted plaintext (`show`, `generate`, `otp`)
  wraps the byte buffer in `zeroize::Zeroizing<_>` so the
  heap allocation scrubs on drop, matching the
  [audit-driven hardening from `723c92b`](../security-audit.md).
  Plaintext on the wire is unavoidable — the extension needs
  to render it — but it doesn't survive in the host's heap.

## Consequences

### Good

- Debuggable: a developer with `tee` between the browser and
  the host can read every request/response pair.
- The op surface is intentionally narrow; widening it requires
  a new ADR, which is the right friction for a remote-control
  channel into a password manager.
- Reuses the existing
  [`Store`](../../crates/bypass-cli/src/main.rs) +
  [`GpgCli`](../../crates/bypass-cli/src/crypto_gpg.rs) +
  [`StorageFs`](../../crates/bypass-cli/src/storage_fs.rs)
  +
  [`Git2Vcs`](../../crates/bypass-cli/src/vcs_git2.rs)
  stack one-to-one — no duplicated logic, no parallel CLI
  surface to keep in sync.
- Forward-compatible: new ops slot into the `op` tag without
  breaking existing requests.

### Bad

- Plaintext crosses the pipe in JSON. Native messaging is a
  local stdin/stdout channel with no network — but a privileged
  local attacker who can attach to the browser process could
  read the pipe. This is the same threat as the CLI's
  `bypass show` output going to a terminal; the browser is now
  the terminal.
- Stringly-typed `op` field: a future rename would be a wire
  break. Mitigated by our owning both sides; if a rename ever
  matters, we ship a new ADR + a transition window.
- The 512 KB cap is a soft ceiling for huge entries (e.g. a
  user storing a 600 KB SSH key blob). Affected users fall
  back to the CLI; documented in the README's
  "Troubleshooting" section.
- No streaming: the host buffers the full reply before
  writing. A 512 KB reply is fine; a 50 MB pull (impossible
  given the cap, but if we ever lifted it) would not be.

## Confirmation

- [`messaging_host.rs`](../../crates/bypass-cli/src/messaging_host.rs)
  is the framing reader/writer + dispatch loop; its tests
  exercise the length-prefix encoding, the op dispatch, and
  the size-cap refusal path.
- The op surface is enumerated by a Rust `enum` — adding an
  op without updating the enum is a compile error.
- The reply-size cap is the `MAX_REPLY_BYTES` constant in
  `messaging_host.rs`; tests pin its current value.
- A future change to the op surface, the error shape, or the
  cap requires a superseding ADR.

## Related ADRs

- [ADR-0001](0001-platform-delegated-crypto.md): the native
  host is the same `bypass` binary that already shells out to
  `gpg`; the extension inherits that crypto delegation.
- [ADR-0009](0009-leak-check-before-push.md): the leak-check
  audit applies if the extension calls `sync` (not in the v1
  op surface), so this ADR doesn't extend or contradict it.
- [ADR-0018](0018-daemon-status-protocol.md): the sync-daemon
  status protocol uses a similar tagged-JSON shape over a Unix
  socket. The two protocols are deliberately not unified —
  different transports, different consumers, different op
  surfaces.
- [ADR-0023](0023-browser-extension-architecture.md): defines
  the extension side that consumes this protocol.
