<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sync-daemon socket location and multi-instance prevention

* Status: accepted; amended by [ADR-0028](0028-drop-macos-support.md) (macOS fallback chain dropped)
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

Phase 5.2.c introduces `bypass sync daemon`, a long-running
foreground process that holds the libp2p Swarm, watches the
store for filesystem changes, and serves `bypass sync status`
queries from the same machine. Two questions need answering
before any code lands:

1. **Where does the daemon listen** for local CLI clients
   (`bypass sync status`, future `bypass sync peer …`
   queries)?
2. **How does the daemon detect** that another `bypass sync
   daemon` is already running on this host, so a second
   `bypass sync daemon` doesn't silently shadow it?

The eval doc flagged this as
[open question #9](../sync-p2p-evaluation.md). This ADR
commits to a concrete answer.

## Considered Options

**Socket location:**

* **`$XDG_RUNTIME_DIR/bypass-sync.sock`** on Linux, with a
  documented fallback chain on platforms where
  `$XDG_RUNTIME_DIR` is unset (chiefly macOS):
  `$TMPDIR/bypass-<uid>-sync.sock` → `/tmp/bypass-<uid>-sync.sock`.
  The runtime-dir variant is automatically per-user, automatically
  swept at logout, and already used by every other modern Linux
  daemon (pulseaudio, pipewire, gnupg).
* `$XDG_DATA_HOME/bypass/sync.sock`. Persistent across reboots
  (wrong — stale sockets accumulate); not auto-cleaned;
  shared if `$XDG_DATA_HOME` is on a network mount.
* TCP loopback (e.g. `127.0.0.1:<port>`). Available without
  Unix sockets, but loses `SO_PEERCRED`-based caller
  authentication and opens an attack surface on shared
  hosts where another user's process can connect to the
  loopback port.
* Per-store socket (under `$PASSWORD_STORE_DIR/.bypass.sock`).
  Tempting because it co-locates daemon state with the store,
  but breaks the "one daemon per device, many stores" pattern
  the daemon design assumes and makes the socket survive
  reboots.

**Multi-instance prevention:**

* **Probe-then-bind on the socket path.** On startup: attempt
  `connect()` to the existing socket. If it succeeds, exit
  non-zero with "already running". If it fails (`ECONNREFUSED`
  → stale socket from a crashed daemon), `unlink()` and re-bind.
* Pidfile. Routinely goes stale across crashes; introduces a
  TOCTOU race between read-pid → check-process → bind.
* Lockfile via `flock`. Works on Linux but `flock` semantics
  vary across filesystems (NFS in particular). Unnecessary
  given the socket is already the liveness probe.

## Decision Outcome

- **Socket path resolution** (in
  [`sync::socket::default_socket_path`](../../crates/bypass-cli/src/sync/socket.rs)):
  1. `$XDG_RUNTIME_DIR/bypass-sync.sock` if `$XDG_RUNTIME_DIR`
     is a non-empty directory we can write to.
  2. Else `$TMPDIR/bypass-<uid>-sync.sock` if `$TMPDIR` is set.
  3. Else `/tmp/bypass-<uid>-sync.sock`.

  `<uid>` is the numeric Unix uid from `nix`/`libc`. The `-<uid>-`
  suffix on the temp-dir variants is what makes the path
  per-user; the runtime-dir variant is already per-user by
  definition.

- **Permissions:** the daemon `chmod`s the socket to `0600`
  immediately after `bind`. On Linux this is belt-and-braces
  (runtime-dir is already `0700`); on macOS/`/tmp` it's the
  only protection against another local user.

- **Multi-instance check:** the daemon calls `UnixStream::connect`
  on the path *before* `UnixListener::bind`. If `connect`
  succeeds, exit code 2 with stderr
  "bypass-sync daemon already running on $PATH (close it first
  with kill -TERM)". If `connect` returns `ECONNREFUSED`
  *or* `ENOENT`, proceed to `bind`; on `ENOENT` after
  `connect-refused`, race recovery is benign because we hold
  the bind and the loser sees `EADDRINUSE`.

- **No pidfile, no lockfile.** The socket is the truth.

- **Windows** is out of scope. Unix-only daemon for v1; the
  pure-`UnixListener` code path is gated behind `#[cfg(unix)]`
  and `bypass sync daemon` returns an explicit "not supported
  on this platform" message on Windows.

## Consequences

### Good

- One canonical location per platform; users debugging a
  weird state can `ls -la $XDG_RUNTIME_DIR/bypass-sync.sock`
  without consulting docs.
- No pidfile bookkeeping; daemon crash leaves at most an
  unlinked socket node, which the next daemon clears
  transparently.
- `SO_PEERCRED` (Linux) / `LOCAL_PEERCRED` (macOS) is
  available to the daemon if it ever needs to authorise the
  caller's uid before answering — a Phase 6 hook, not
  required for 5.2.c.
- The socket doubles as a liveness probe: any client (or a
  future systemd unit) can `connect` to know whether the
  daemon is up.

### Bad

- `$XDG_RUNTIME_DIR` is conventionally tmpfs on Linux; the
  socket inode dies with the boot session, so a daemon
  restarted after a kernel oops sees `ENOENT` and re-binds.
  That's fine semantically but means *between* reboots the
  socket path is unstable. Documented as expected behaviour;
  no UX impact.
- Race between two `bypass sync daemon` starts: both call
  `connect`, both get `ECONNREFUSED`, both `unlink`, both
  `bind`. The winner of the second `bind` wins; the loser
  gets `EADDRINUSE` and exits cleanly. We rely on the kernel
  to serialise. No data integrity risk — neither has accepted
  a client yet.

## Confirmation

- `sync::socket::default_socket_path` returns the documented
  fallback chain. Unit tests with `XDG_RUNTIME_DIR` /
  `TMPDIR` manipulated through `std::env::set_var` verify the
  precedence.
- `sync::socket::bind_or_refuse_existing` is the
  probe-then-bind function. Unit test: start a listener,
  attempt a second bind, assert it returns the
  `DaemonAlreadyRunning` error variant.
- The two-process daemon integration test in
  `crates/bypass-cli/tests/sync_daemon.rs` is the end-to-end
  confirmation that the socket is actually reachable from a
  second `bypass` process.

## Related ADRs

- [ADR-0009](0009-leak-check-before-push.md): same
  refuse-by-default posture (here for second-daemon, there
  for plaintext push).
- [ADR-0015](0015-device-identity-key.md): also lives under
  `$XDG_CONFIG_HOME`; the socket lives under
  `$XDG_RUNTIME_DIR` instead because it's ephemeral state,
  not durable identity.
- [ADR-0018](0018-daemon-status-protocol.md): defines the
  protocol that runs over this socket.
