<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Sync-daemon service supervision: systemd user unit + launchd agent

* Status: macOS portion superseded by [ADR-0028](0028-drop-macos-support.md); Linux portion remains accepted
* Date: 2026-05-22
* Deciders: hiroshiyui

## Context and Problem Statement

Phase 5.2.c shipped `bypass sync daemon` as a foreground
process. The user runs it from a terminal, the daemon serves
inbound RPCs and watches the store, the user
`Ctrl-C`s when they're done. That works for evaluation; it
doesn't survive a reboot, doesn't auto-restart on crash, and
needs the user to remember to start it after every login.

The eval doc flagged this as
[open question #2](../sync-p2p-evaluation.md) and deferred the
service-management glue to Phase 6. Time to land it. The
ROADMAP already prescribes the shape: a systemd user unit on
Linux, a launchd user agent on macOS, both at conventional
paths, both driven by a small `bypass sync daemon <op>` CLI
surface.

Two questions need answering:

1. **What runs the daemon?** systemd, launchd, or do we
   re-invent supervision?
2. **What's the CLI surface?** Install, uninstall, start, stop,
   enable, disable, status — and how do they relate to the
   existing `bypass sync status`?

## Considered Options

**Supervisor:**

* **Platform-native (systemd `--user` on Linux, launchd user
  agent on macOS).** Auto-restart, login persistence,
  centralised status, journalctl-style logs. Every modern
  desktop user already has them running for other long-lived
  per-user services (pulseaudio, gnome-keyring, syncthing,
  …). No code to maintain on our side beyond the unit / plist
  template.
* A custom supervisor written in Rust. We'd own restart logic,
  log rotation, dependency-on-display-server-availability,
  resource limits. Not where our complexity budget should
  go.
* A wrapper script in `~/.profile` that backgrounds `bypass
  sync daemon &`. Survives the first login but not crashes;
  surfaces no status; not a real answer.

**Unit / plist install location:**

* `~/.config/systemd/user/bypass-sync.service` and
  `~/Library/LaunchAgents/io.bypass.sync.plist`. The
  conventional per-user paths. Both `systemctl --user
  daemon-reload` and `launchctl bootstrap` discover them
  automatically.
* System-wide units (`/etc/systemd/system/`,
  `/Library/LaunchDaemons/`). Need root; one-daemon-per-user
  with per-user `peers.toml` etc. argues against system-wide.
  We never want root to start a process that reads a user's
  password store.

**Auto-start posture:**

* **Off by default.** `bypass sync daemon install` writes the
  unit but does not enable it. The user explicitly opts in
  with `enable`. Matches the rate-limit / leak-check
  refuse-by-default posture
  ([ADR-0009](0009-leak-check-before-push.md),
  [ADR-0016](0016-sync-dos-defences.md)): nothing background-y
  starts without user intent.
* Auto-enable on install. Surprising; one `install` step ends
  with a daemon running at next boot that the user didn't
  ask for.

**Status semantics — `bypass sync daemon status` vs `bypass sync
status`:**

* **They report different things, both useful.** `bypass sync
  daemon status` asks the supervisor: "is the daemon currently
  running, is it enabled to auto-start, what was its last exit
  code?" `bypass sync status` asks the live daemon over the
  Unix socket: "which peers can you see, when did you last
  sync, what's your peer-id?" The first is a process-state
  query; the second is an application-state snapshot. Both
  survive without the other (`bypass sync status` works for a
  foreground daemon too; `daemon status` doesn't care whether
  the daemon ever served a request).
* Merge them under one command. Confusing — the daemon could
  legitimately be "running" (supervisor says yes) but
  "unresponsive" (socket connect times out), or "stopped" but
  with stale state still on disk. Keeping them separate makes
  the failure modes explicit.

## Decision Outcome

- **Supervisor:** systemd user unit on Linux; launchd user
  agent on macOS. No custom supervision logic.

- **Install paths**:
  - Linux: `~/.config/systemd/user/bypass-sync.service`.
  - macOS: `~/Library/LaunchAgents/io.bypass.sync.plist`.
  Resolved via `dirs::home_dir()` (not `sync::config_dir()` —
  the systemd path is a fixed convention, not under our
  XDG-bypass subtree).

- **Unit body** (Linux):
  ```ini
  [Unit]
  Description=bypass-sync LAN peer-to-peer sync daemon
  Documentation=https://github.com/hiroshiyui/bypass
  After=network-online.target

  [Service]
  Type=simple
  ExecStart=<absolute path from std::env::current_exe()> sync daemon
  Restart=on-failure
  RestartSec=10
  Environment=RUST_LOG=info

  [Install]
  WantedBy=default.target
  ```
  The `ExecStart` path is baked in at install time, the same
  way [`sync::merge_driver::register_in_git_config`](../../crates/bypass-cli/src/sync/merge_driver.rs)
  bakes the merge-driver path into `.git/config`.

- **Plist body** (macOS):
  ```xml
  <?xml version="1.0" encoding="UTF-8"?>
  <!DOCTYPE plist …>
  <plist version="1.0">
  <dict>
    <key>Label</key>      <string>io.bypass.sync</string>
    <key>ProgramArguments</key>
      <array>
        <string><absolute current_exe()></string>
        <string>sync</string>
        <string>daemon</string>
      </array>
    <key>RunAtLoad</key>  <false/>
    <key>KeepAlive</key>
      <dict>
        <key>SuccessfulExit</key> <false/>
      </dict>
    <key>StandardOutPath</key>  <string>/tmp/bypass-sync.log</string>
    <key>StandardErrorPath</key><string>/tmp/bypass-sync.log</string>
  </dict>
  </plist>
  ```

- **Auto-start posture:** off until `bypass sync daemon enable`.
  `install` just writes the file and runs `systemctl --user
  daemon-reload` (or the launchd equivalent). It does not
  start the daemon or enable autostart.

- **CLI surface** (extends `cli::SyncCmd::Daemon` to carry an
  optional sub-action):
  ```
  bypass sync daemon                 # foreground (existing 5.2.c behaviour)
  bypass sync daemon install         # write unit / plist
  bypass sync daemon uninstall       # remove it
  bypass sync daemon start           # supervisor-managed start
  bypass sync daemon stop            # supervisor-managed stop
  bypass sync daemon enable          # autostart on login
  bypass sync daemon disable         # don't autostart
  bypass sync daemon status          # is the supervisor running it?
  ```
  Service `status` is distinct from `bypass sync status` (the
  socket query from [ADR-0018](0018-daemon-status-protocol.md)):

  | Command                     | Question                                                                |
  |-----------------------------|-------------------------------------------------------------------------|
  | `bypass sync daemon status` | "Is the supervisor running the daemon? What was its last exit code?"    |
  | `bypass sync status`        | "What does the running daemon see — peers, listen addrs, last syncs?"   |

- **Windows is out of scope** for v1. The daemon itself is
  already `#[cfg(unix)]`; service installation is gated the
  same way and surfaces an explicit "not supported on this
  platform" error.

## Consequences

### Good

- Reboot- and crash-survivable peer sync, the same way every
  other modern desktop background service works.
- The implementation is ~150 lines: a unit template, a plist
  template, and seven small functions wrapping `systemctl
  --user` / `launchctl` invocations. Real complexity stays in
  the OS.
- The off-by-default install matches the rest of `bypass`'s
  refuse-by-default UX. Users who didn't ask for autostart
  don't get it.
- `bypass sync daemon status` + `bypass sync status` together
  cover both failure modes the user can encounter (supervisor
  thinks it's running but the socket is gone → the daemon
  hung; supervisor says stopped → restart it).

### Bad

- Two different supervisors mean two different unit-body
  formats to maintain. The blast radius is small (~30 lines
  each) but Linux/macOS divergence is real.
- launchd's `KeepAlive { SuccessfulExit: false }` is *similar*
  to systemd's `Restart=on-failure` but not identical:
  launchd considers an exit code of 0 a "successful" exit, so
  a daemon that exits 0 on a fatal-but-tidy condition won't
  be restarted on macOS even when it would be on Linux. Our
  daemon currently only exits 0 on SIGTERM / SIGINT
  (intentional shutdown), which matches the desired
  behaviour, but a future change to exit 0 on a recoverable
  error would diverge silently between platforms. Tracked in
  the daemon module's docstring.
- macOS launchd does not have a clean equivalent of journalctl,
  so the plist redirects stdout + stderr to
  `/tmp/bypass-sync.log` rather than letting them disappear.
  That file isn't rotated; a long-running daemon will grow it.
  Phase 6.x or a later ADR can swap in a rotation strategy.

## Confirmation

- `crates/bypass-cli/src/sync/service.rs` ships in the same
  commit series as this ADR. Unit tests render the unit / plist
  templates with a fixed `current_exe()` substitute and assert
  the output contains the expected directives, without
  invoking `systemctl` / `launchctl`.
- The CLI surface
  ([`SyncCmd::Daemon { sub: Option<SyncDaemonCmd> }`](../../crates/bypass-cli/src/cli.rs))
  surfaces every op in `bypass sync daemon --help`.
- Resolves
  [eval-doc OQ #2](../sync-p2p-evaluation.md#open-questions-to-resolve-before-implementation).

## Related ADRs

- [ADR-0015](0015-device-identity-key.md): the per-user
  identity key. This ADR's per-user supervision posture is
  the natural complement.
- [ADR-0017](0017-daemon-socket-location.md): the
  status-socket location. Same per-user, runtime-dir
  convention.
- [ADR-0018](0018-daemon-status-protocol.md): defines the
  `bypass sync status` socket protocol that `bypass sync
  daemon status` deliberately does *not* duplicate.
