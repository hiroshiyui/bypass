<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sync-daemon status protocol and `bypass sync status` output shape

* Status: accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

[ADR-0017](0017-daemon-socket-location.md) commits to a Unix
socket as the daemon's local-IPC surface. This ADR commits to
the *protocol* spoken over that socket and to what
`bypass sync status` actually prints. The eval doc named this as
[open question #10](../sync-p2p-evaluation.md): "connected peers,
most-recent sync timestamp per peer, pending conflicts list — what
shape does the UI have?"

The need is small but immediate:

- Users have no way today to tell whether a paired peer is on
  the LAN.
- The auto-rebase semantics (ADR-0011) need an audit trail — a
  user surprised by a fast-forward should be able to ask the
  daemon "what did you do recently?"
- Future ops (peer-revoke, force-sync, pause-sync) want a
  protocol they can extend without re-litigating the wire shape.

## Considered Options

**Wire format:**

* **Newline-delimited JSON.** One JSON object per request,
  one per reply, both terminated by `\n`. Trivial to parse,
  trivial to debug (`socat - UNIX-CONNECT:$path`), the same
  format the rest of `bypass` already serialises with
  (`serde_json` is already a dep).
* CBOR / postcard. Smaller on the wire, but the *whole
  reason* to expose a debuggable local IPC is so a user can
  poke at it with stock tools.
* gRPC / Cap'n Proto / Protobuf. Wildly over-engineered for
  a single-host, single-binary protocol.

**Per-connection lifecycle:**

* **One request, one reply, close.** Simplest possible.
  Connection state lives in the client; daemon state lives
  in the daemon's RAM.
* Long-lived connection with multiple ops. Useful for a TUI
  ("watch -n1 bypass sync status"), but adds framing and
  cancellation that 5.2.c doesn't need. Add later if a real
  TUI lands.

**Op surface:**

* **One `status` op in 5.2.c.** The smallest useful surface.
  Future ADRs can extend; until they do, the unknown-op
  branch returns `{"error":"unknown op"}` and closes.
* Anticipate every op now. Premature; we'd guess wrong.

**Reply shape:**

* **Local peer-id, listening addrs, and one struct per peer**
  with: pinned name, peer-id, `discovered` (is mDNS seeing it
  right now?), the last `SyncAction` we logged for this peer
  (variant name as a string for forward-compat), and a unix
  timestamp of that action.
* Also include pending conflicts. ADR-0011's auto-rebase
  resolves opaque-blob conflicts via the merge driver; there
  are no *pending* conflicts to surface in 5.2.c. Add when
  manual-resolution fallback ships (post-5.2.c).
* Connection counts, byte counters, libp2p ping RTT.
  Diagnostic noise; not useful at this layer of abstraction.

**Console rendering:**

* Small fixed-width table by default. `--json` emits the raw
  reply for scripts and tests.

## Decision Outcome

- **Wire format:** newline-delimited JSON over the
  [ADR-0017](0017-daemon-socket-location.md) Unix socket. The
  request is a single line; the reply is a single line; the
  daemon closes the connection after writing.

- **Request schema** (serde-tagged):
  ```json
  {"op": "status"}
  ```
  Any other shape, or any unknown `op`, gets:
  ```json
  {"error": "unknown op"}
  ```

- **Reply schema** (success):
  ```json
  {
    "local_peer_id": "12D3KooW…",
    "listening_addrs": ["/ip4/192.168.1.42/tcp/45678"],
    "peers": [
      {
        "name": "phone",
        "peer_id": "12D3KooW…",
        "discovered": true,
        "last_sync_action": "FastForwarded",
        "last_sync_unix": 1779410123
      }
    ]
  }
  ```
  - `last_sync_action` is the string name of the relevant
    [`SyncAction`](../../crates/bypass-cli/src/sync/syncing.rs)
    variant (`"UpToDate"`, `"FastForwarded"`, `"PeerBehind"`,
    `"Rebased"`, `"AwaitingPeerRebase"`, `"RejectedLeak"`), or
    `null` if we've never sync'd with this peer.
  - `last_sync_unix` mirrors `last_sync_action`; both are
    `null` together.
  - `discovered` reflects whether mDNS currently sees the
    peer on the LAN, not whether we have a live connection.

- **Console rendering:** `bypass sync status` prints a small
  table:
  ```
  Daemon: 12D3KooW…abc
  Listening: /ip4/192.168.1.42/tcp/45678
  Peers:
    phone     12D3KooW…xyz   discovered=yes   last=FastForwarded (3m ago)
    laptop    12D3KooW…def   discovered=no    last=(never)
  ```
  `bypass sync status --json` emits the raw reply line for
  scripts and integration tests.

- **Error reporting:** if the daemon is not running
  (`connect` returns `ENOENT` / `ECONNREFUSED`), `bypass sync
  status` exits 1 with stderr "daemon not running (start it
  with `bypass sync daemon`)". This is the only client-side
  failure mode that needs a friendly message.

## Consequences

### Good

- Debuggable with stock tools (`socat`, `nc -U`); no
  protobuf-decoding excursions.
- Forward-compatible: new ops slot into the `op` tag without
  changing the wire format.
- Test surface is small: a fake socket end via
  `tokio::io::duplex` exercises the whole protocol; the
  two-process daemon test confirms the real socket path.
- `--json` makes the daemon scriptable today, before any
  bypass-specific tooling exists.

### Bad

- One-request-one-reply means a "watch status" UX has to
  poll. Acceptable for 5.2.c; if a real TUI lands, we add a
  `subscribe` op rather than retrofitting framing.
- Stringly-typed `last_sync_action`: a future
  `SyncAction` rename is a wire break. Mitigated by serde
  enum names matching the Rust variant names by default and
  the daemon owning both ends in v1.
- No auth on the socket. ADR-0017's `0600` perms +
  per-user runtime dir do the heavy lifting; a multi-user
  host with one user running the daemon and another with shell
  access is out of threat-model scope.

## Confirmation

- The `StatusSnapshot` serde struct in
  [`sync::socket`](../../crates/bypass-cli/src/sync/socket.rs)
  is the wire schema; round-trip unit tests assert
  request/reply parse the way this ADR documents.
- `bypass sync status` integration coverage: the daemon
  test in `crates/bypass-cli/tests/sync_daemon.rs` runs
  `bypass sync status --json` against a live daemon and
  parses the JSON.

## Related ADRs

- [ADR-0017](0017-daemon-socket-location.md): defines the
  socket this protocol runs on.
- [ADR-0011](0011-sync-semantics-hybrid.md): defines the
  `SyncAction` variants surfaced in the reply.
- [ADR-0019](0019-peer-revocation-trust-semantics.md):
  documents how revocation interacts with the `peers` array
  (revoked peers stop appearing).
