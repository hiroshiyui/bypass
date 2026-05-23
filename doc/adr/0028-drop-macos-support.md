<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Drop macOS as a supported target

* Status: accepted
* Date: 2026-05-23
* Deciders: hiroshiyui

## Context and Problem Statement

`bypass` has carried macOS as a first-class Linux peer since
[ADR-0020](0020-daemon-service-supervision.md) decided service
supervision via launchd, [ADR-0017](0017-daemon-socket-location.md)
spelled out a macOS socket-path fallback chain, and
[ADR-0021](0021-release-packaging.md) committed to building
`x86_64-apple-darwin` + `aarch64-apple-darwin` artefacts on every
tag. Phase 6's CI matrix runs the full test suite on
`macos-latest`. Phase 7 (browser extension) carries a macOS branch
in `native_host_install.rs` for Chrome's per-user
NativeMessagingHosts directory.

That posture is costing more than it's worth:

* **The sole developer does not use macOS.** No personal vault on
  Darwin, no manual smoke-test path, no day-one user beyond CI.
* **FSEvents brittleness has been a recurring source of CI work.**
  Commit `60044e9` added a canonicalize step to fix one FSEvents
  symlink-resolution quirk; this week another CI run failed on
  `writes_under_dot_git_do_not_trigger_a_tick` (FSEvents appears to
  deliver a coalesced event with the watched root as the path,
  bypassing the `.git/` filter). The class of bug keeps producing
  new instances, each requiring a defensive code change with no
  way to repro locally.
* **The launchd half of [ADR-0020](0020-daemon-service-supervision.md)
  is parallel-to-but-not-equivalent-to the systemd half** — the
  ADR's own "Bad" section flags `KeepAlive { SuccessfulExit: false }`
  vs `Restart=on-failure` divergence and the lack of journalctl on
  macOS (stdout/stderr redirected to an unrotated
  `/tmp/bypass-sync.log`). Carrying both supervisors means a future
  daemon exit-code change can silently break one platform.
* **Release packaging spends two of four matrix slots on Darwin**
  ([ADR-0021](0021-release-packaging.md)), doubling the surface for
  release-time regressions on a target with no users.

We are still pre-1.0. Trimming scope now is cheaper than carrying
it through v1 and removing later.

## Considered Options

* **A. Keep macOS, accept the maintenance overhead.** Status quo.
  Every FSEvents flake gets a defensive patch; launchd divergence
  gets watched manually; release artefacts ship that nobody asks
  for.
* **B. Keep macOS on CI only, no release artefacts.** Halves the
  release matrix cost but keeps the source-code branches alive and
  preserves the FSEvents bug-discovery treadmill. The supervision
  glue still needs both arms.
* **C. Drop macOS entirely.** No `cfg(target_os = "macos")` arms,
  no `macos-latest` CI runner, no darwin release targets, no
  launchd plist generator. README, ROADMAP, CHANGELOG, and the
  affected ADRs all reflect Linux-only.

## Decision Outcome

Chosen option: **C — drop macOS entirely.**

Concretely:

* **Code.** Delete the macOS arms in
  [`crates/bypass-cli/src/sync/service.rs`](../../crates/bypass-cli/src/sync/service.rs)
  (launchd plist generation, `launchctl` wrappers, the `macos`-gated
  tests). Simplify
  [`crates/bypass-cli/src/sync/watcher.rs`](../../crates/bypass-cli/src/sync/watcher.rs)
  by removing the FSEvents-driven `canonicalize()` step
  (inotify event paths already match the watch root). Drop the
  `$TMPDIR` / `/tmp` fallback chain from
  [`crates/bypass-cli/src/sync/socket.rs`](../../crates/bypass-cli/src/sync/socket.rs)
  — `$XDG_RUNTIME_DIR` is required on Linux, and its absence is
  now a hard error rather than a silent fallback. Remove the macOS
  Chrome `NativeMessagingHosts` branch from
  [`crates/bypass-cli/src/native_host_install.rs`](../../crates/bypass-cli/src/native_host_install.rs).
  Strip `cfg(target_os = "macos")` from the end-to-end test
  suite.
* **CI.** Drop `macos-latest` from the test matrix in
  [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml) and
  delete the "Install GPG (macOS only)" step.
* **Release.** Drop `x86_64-apple-darwin` and `aarch64-apple-darwin`
  from [`.github/workflows/release.yml`](../../.github/workflows/release.yml).
  v0.1.x ships two artefacts: `x86_64-unknown-linux-gnu` and
  `aarch64-unknown-linux-gnu`.
* **Docs.** Update README, CHANGELOG, ROADMAP, CLAUDE.md, and
  [`doc/sync-p2p-evaluation.md`](../sync-p2p-evaluation.md) to
  reflect Linux-only support. Help-text for
  `bypass sync daemon install` no longer mentions launchd.
* **Browser extension** (Phase 7, not yet shipped) is Linux-only
  on first release. Manifest V3 ([ADR-0023](0023-browser-extension-architecture.md))
  itself is OS-agnostic; only the native-messaging-host install
  path is affected, and only for the desktop binary.

### What this supersedes / amends

* **[ADR-0020](0020-daemon-service-supervision.md):** the launchd
  half is **superseded**. The chosen supervisor is now systemd user
  unit only; the plist template and `launchctl` wrappers documented
  there no longer apply. ADR-0020's `Bad` notes about
  Linux/macOS divergence become moot. The Linux half of the
  decision stands unchanged. ADR-0020's status is updated to
  `Superseded by ADR-0028 (macOS portion); Linux portion remains
  accepted.`
* **[ADR-0017](0017-daemon-socket-location.md):** **amended** to
  drop the `$TMPDIR` / `/tmp/bypass-<uid>-sync.sock` fallback
  chain. The macOS-specific `chmod 0600` belt-and-braces line
  becomes plain belt — runtime-dir is already `0700` on Linux —
  but we keep the explicit `chmod 0600` because the cost is one
  syscall and the property is worth asserting at the source.
  ADR-0017's `Considered Options` and `Decision Outcome` sections
  read as if `$XDG_RUNTIME_DIR` was always the only path; the
  fallback was a portability concession to macOS that is no longer
  needed.
* **[ADR-0021](0021-release-packaging.md):** **amended** to drop
  the two darwin targets from the v0.1.x matrix. The remaining
  matrix is `x86_64-unknown-linux-gnu` + `aarch64-unknown-linux-gnu`
  (the latter still via `cross` on `ubuntu-latest`). The
  `macos-latest` runner is no longer referenced.

Older ADRs that mention macOS only in passing
([ADR-0015](0015-device-identity-key.md) for the
`$XDG_CONFIG_HOME` fallback wording, security-audit notes on
`arboard`) keep their text — the wording is still factually
correct in the cross-platform abstract even though we no longer
ship for macOS, and rewriting them adds churn for no gain.

### Reasoning

* **The bug rate isn't going to decrease.** FSEvents quirks are a
  property of macOS, not our code. We've patched two distinct
  symptoms in two months; the next one is a matter of when, not
  if. A platform that costs more than it earns at pre-1.0 is a
  platform to drop.
* **Linux is the natural home for a pass-compatible password
  manager.** `pass` itself is Linux-first; the GPG toolchain, the
  XDG conventions, and the systemd ecosystem all match `bypass`'s
  shape without translation. macOS support was always a
  portability nice-to-have, not a target requirement.
* **Pre-1.0 is the cheapest time to do this.** No external users
  to migrate, no published Homebrew tap to deprecate, no
  third-party packagers to coordinate with. The CHANGELOG flips
  from "Linux + macOS" to "Linux" between un-released cuts.
* **Windows remains out of scope** (no change). The daemon is
  still `#[cfg(unix)]`. Removing macOS does not narrow the
  long-term option to add it back later via a focused ADR if a
  real contributor wants to do that work.

## Consequences

### Good

* `crates/bypass-cli/src/sync/service.rs` shrinks to a single
  supervisor implementation — no `cfg`-fragmented arms, no
  divergent exit-code semantics, no plist XML to maintain.
* Watcher and socket modules drop their macOS workaround code,
  making the inotify-only path the only path.
* CI runs faster (one OS, not two) and is more predictable
  (FSEvents drops out of the failure surface).
* Release pipeline halves: two targets, both Linux, both with a
  developer who actually runs them.
* Documentation can speak in concrete Linux conventions (systemd,
  XDG, inotify, `/proc`) instead of always disclaiming "on Linux,
  …; on macOS, …".

### Bad

* Any users who built `bypass` from source on macOS lose support.
  Mitigation: pre-1.0, no announced macOS users; if one surfaces
  they can pin to a tag predating this ADR, but we won't be
  fixing macOS bugs on `main`.
* The architectural discipline of "platform-delegated crypto"
  ([ADR-0001](0001-platform-delegated-crypto.md)) and
  "platform-agnostic core" ([ADR-0003](0003-workspace-split-core-cli.md))
  loses one of its proof points — `bypass-cli` no longer
  demonstrates that the core/frontend split works across two
  desktop OSes. The split is still load-bearing for Android
  ([ADR-0024](0024-android-ffi-via-uniffi.md)) and the browser
  extension ([ADR-0023](0023-browser-extension-architecture.md)),
  so the discipline doesn't slacken — it's just less visible at
  the desktop layer.
* Adding macOS back later (if a real contributor wants to) means
  re-introducing the launchd plist, the FSEvents canonicalize
  step, the socket fallback chain, and the Chrome
  `NativeMessagingHosts` path. The deletions in this ADR's commit
  series are recoverable from git, but a fresh
  re-introduction needs a new ADR superseding this one.

### Confirmation

* Code: `grep -rn 'macos\|launchd\|fsevent\|darwin' crates/`
  returns no hits after the cleanup commit lands (modulo
  documentation comments that survive intentionally — e.g.,
  ADR cross-references in this file).
* CI: `.github/workflows/ci.yml` matrix has one entry
  (`ubuntu-latest`); `.github/workflows/release.yml` strategy
  matrix has two entries, both `unknown-linux-gnu`.
* Docs: README's "Platforms" / Phase-status section names Linux
  only; CHANGELOG carries a `Removed` entry under the next
  unreleased section.

## Related ADRs

* [ADR-0017](0017-daemon-socket-location.md): amended (drops the
  macOS fallback chain).
* [ADR-0020](0020-daemon-service-supervision.md): superseded in
  part (the launchd half).
* [ADR-0021](0021-release-packaging.md): amended (drops the two
  darwin targets).
* [ADR-0001](0001-platform-delegated-crypto.md) /
  [ADR-0003](0003-workspace-split-core-cli.md): the core/frontend
  split still holds; only one frontend ships for now, but the
  Android FFI and browser extension still exercise the discipline.
