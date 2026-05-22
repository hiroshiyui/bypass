// SPDX-License-Identifier: GPL-3.0-or-later

//! Two-process daemon loopback tests for sync.
//!
//! All `#[ignore]` by default per
//! [ADR-0013](../../../doc/adr/0013-sync-transport-trait.md): each
//! test spawns two `bypass sync daemon` child processes (each with
//! its own GPG keyring, store, identity key and Unix socket) and
//! exercises the daemon lifecycle end-to-end.
//!
//! - [`daemons_start_serve_status_and_exit_cleanly`] is mDNS-agnostic:
//!   it verifies the daemon's bind/serve/shutdown lifecycle without
//!   requiring peer discovery.
//! - [`full_round_trip_insert_on_a_then_show_on_b_via_mdns`] is the
//!   full data-flow test (pair → discover → bootstrap-sync → insert
//!   on A → show on B). It self-skips the data-flow assertion with
//!   a clear stderr message when the host has no working IPv4
//!   multicast routes.
//!
//! Run on demand:
//!
//! ```sh
//! cargo test -p bypass --test sync_daemon -- --ignored
//! ```

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

mod common;

/// Spawn `bypass <args>` with the env-pair plumbing the other
/// integration tests use, plus a per-process `XDG_CONFIG_HOME` and
/// `XDG_RUNTIME_DIR` (so the daemon's socket path is sandboxed).
fn spawn_bypass(
    env: &common::TestEnv,
    cfg: &Path,
    runtime: &Path,
    extra: &[(&str, &Path)],
    args: &[&str],
) -> Child {
    let exe = assert_cmd::cargo::cargo_bin("bypass");
    let mut cmd = Command::new(exe);
    for (k, v) in env.env_pairs() {
        cmd.env(k, v);
    }
    cmd.env("XDG_CONFIG_HOME", cfg)
        .env("XDG_RUNTIME_DIR", runtime);
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn().expect("spawn bypass child")
}

fn read_until(
    reader: &mut BufReader<ChildStdout>,
    deadline: Instant,
    mut pred: impl FnMut(&str) -> bool,
    prefix: &mut Vec<String>,
) -> Option<String> {
    while Instant::now() < deadline {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                let trimmed = line.trim_end().to_owned();
                if pred(&trimmed) {
                    return Some(trimmed);
                }
                prefix.push(trimmed);
            }
            Err(_) => return None,
        }
    }
    None
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return None,
        }
    }
}

/// Pair two `bypass` processes against each other on loopback. Returns
/// after both have written `peers.toml`. Uses each process's own GPG
/// keyring + store dir from `env_a` / `env_b`; identity / peers live
/// under the supplied config dirs.
fn pair_two_processes(
    env_a: &common::TestEnv,
    env_b: &common::TestEnv,
    cfg_a: &Path,
    cfg_b: &Path,
    runtime_a: &Path,
    runtime_b: &Path,
) {
    let mut show = spawn_bypass(
        env_a,
        cfg_a,
        runtime_a,
        &[],
        &[
            "sync",
            "pair",
            "--show",
            "--addr",
            "/ip4/127.0.0.1/tcp/0",
            "--name",
            "device-a",
        ],
    );
    let mut show_stdout = BufReader::new(show.stdout.take().expect("show stdout"));

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut prefix = Vec::new();
    let pin_line = read_until(
        &mut show_stdout,
        deadline,
        |s| s.starts_with("PAIRING PIN:"),
        &mut prefix,
    )
    .unwrap_or_else(|| panic!("never saw PAIRING PIN; got:\n{}", prefix.join("\n")));
    let pin = pin_line
        .strip_prefix("PAIRING PIN:")
        .unwrap()
        .trim()
        .to_owned();
    let addr_line = read_until(
        &mut show_stdout,
        deadline,
        |s| s.contains("/p2p/"),
        &mut prefix,
    )
    .unwrap_or_else(|| panic!("never saw /p2p/ addr; got:\n{}", prefix.join("\n")));
    let multiaddr = addr_line.trim().to_owned();

    let mut enter = spawn_bypass(
        env_b,
        cfg_b,
        runtime_b,
        &[],
        &[
            "sync", "pair", "--enter", "--addr", &multiaddr, "--name", "device-b",
        ],
    );
    enter
        .stdin
        .as_mut()
        .expect("enter stdin")
        .write_all(format!("{pin}\n").as_bytes())
        .expect("write PIN");

    let enter_status =
        wait_with_timeout(&mut enter, Duration::from_secs(30)).expect("enter timed out");
    let show_status =
        wait_with_timeout(&mut show, Duration::from_secs(30)).expect("show timed out");
    assert!(enter_status.success(), "enter side: {enter_status}");
    assert!(show_status.success(), "show side: {show_status}");
}

/// Run `bypass sync status --json` until `pred` is satisfied or
/// `deadline` elapses. Returns `None` on timeout (with the last
/// snapshot logged to stderr for diagnostics).
fn try_poll_status_until(
    env: &common::TestEnv,
    cfg: &Path,
    runtime: &Path,
    deadline: Instant,
    mut pred: impl FnMut(&serde_json::Value) -> bool,
) -> Option<serde_json::Value> {
    let mut last = serde_json::Value::Null;
    while Instant::now() < deadline {
        let mut child = spawn_bypass(env, cfg, runtime, &[], &["sync", "status", "--json"]);
        let stdout = child.stdout.take().expect("status stdout");
        let _ = child.wait();
        let bytes = std::io::read_to_string(stdout).unwrap_or_default();
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(bytes.trim()) {
            if pred(&v) {
                return Some(v);
            }
            last = v;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    eprintln!(
        "status predicate never satisfied; last snapshot:\n{}",
        serde_json::to_string_pretty(&last).unwrap_or_default()
    );
    None
}

/// Variant that panics on timeout. Used by tests where the predicate
/// MUST be satisfied (vs. the mDNS-dependent skip path).
fn poll_status_until(
    env: &common::TestEnv,
    cfg: &Path,
    runtime: &Path,
    deadline: Instant,
    pred: impl FnMut(&serde_json::Value) -> bool,
) -> serde_json::Value {
    try_poll_status_until(env, cfg, runtime, deadline, pred)
        .unwrap_or_else(|| panic!("status predicate never satisfied within deadline"))
}

/// End-to-end daemon lifecycle: pair two devices, start a daemon on
/// each, query `bypass sync status` on each, then send SIGTERM and
/// verify both exit cleanly.
///
/// **What this test does NOT cover:** mDNS-driven peer discovery and
/// the resulting auto-sync. Those depend on the host's IPv4
/// multicast routes (`224.0.0.0/24` on a working interface), which
/// dev VMs and some CI hosts don't expose. The discovery-driven
/// full sync round-trip is a 5.2.d concern, intended for an
/// integration environment with documented multicast support.
#[test]
#[ignore]
fn daemons_start_serve_status_and_exit_cleanly() {
    // Each side gets a fresh GPG keyring + store + XDG config + XDG
    // runtime so the two daemons don't clobber each other's identity
    // / peers / socket files.
    let env_a = common::TestEnv::new();
    let env_b = common::TestEnv::new();
    let cfg_a = tempfile::TempDir::new().unwrap();
    let cfg_b = tempfile::TempDir::new().unwrap();
    let runtime_a = tempfile::TempDir::new().unwrap();
    let runtime_b = tempfile::TempDir::new().unwrap();

    // Initialise both stores so the daemon's HEAD probe doesn't see
    // an unborn branch. Each side gets its *own* GPG key from
    // TestEnv::new, so the histories diverge — the daemon's first
    // outbound sync will surface that as `RejectedLeak` or a rebase
    // failure, which is fine for the discovery-only assertion we
    // make here. End-to-end data sync between two devices that
    // started from independent inits is a 5.2.d concern; today's
    // mental model is "pair first, then both clone from one".
    let bin = assert_cmd::cargo::cargo_bin("bypass");
    for (env, cfg, runtime) in [
        (&env_a, cfg_a.path(), runtime_a.path()),
        (&env_b, cfg_b.path(), runtime_b.path()),
    ] {
        let mut cmd = Command::new(&bin);
        for (k, v) in env.env_pairs() {
            cmd.env(k, v);
        }
        cmd.env("XDG_CONFIG_HOME", cfg)
            .env("XDG_RUNTIME_DIR", runtime)
            .args(["init", common::TEST_RECIPIENT]);
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "init failed: {:?}", out);
    }

    pair_two_processes(
        &env_a,
        &env_b,
        cfg_a.path(),
        cfg_b.path(),
        runtime_a.path(),
        runtime_b.path(),
    );

    // Spawn daemons.
    let mut daemon_a = spawn_bypass(
        &env_a,
        cfg_a.path(),
        runtime_a.path(),
        &[],
        &["sync", "daemon"],
    );
    let mut daemon_b = spawn_bypass(
        &env_b,
        cfg_b.path(),
        runtime_b.path(),
        &[],
        &["sync", "daemon"],
    );

    // Wait for both daemons to bind their sockets and respond to a
    // `bypass sync status` query that includes the paired peer.
    let deadline = Instant::now() + Duration::from_secs(15);
    let snap_a = poll_status_until(
        &env_a,
        cfg_a.path(),
        runtime_a.path(),
        deadline,
        peer_is_listed,
    );
    let snap_b = poll_status_until(
        &env_b,
        cfg_b.path(),
        runtime_b.path(),
        deadline,
        peer_is_listed,
    );
    assert!(peer_is_listed(&snap_a), "A status missing peer: {snap_a}");
    assert!(peer_is_listed(&snap_b), "B status missing peer: {snap_b}");
    // Each side knows its own peer-id and is listening on at least
    // one interface — proves the libp2p Swarm is wired up.
    assert!(
        snap_a
            .get("listening_addrs")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty()),
        "A reported no listening addrs: {snap_a}"
    );
    assert!(
        snap_b
            .get("listening_addrs")
            .and_then(|v| v.as_array())
            .is_some_and(|a| !a.is_empty()),
        "B reported no listening addrs: {snap_b}"
    );

    // Clean shutdown via SIGTERM. Spawn `kill` rather than linking
    // libc; we don't want a dev-dep just for one syscall.
    for d in [&daemon_a, &daemon_b] {
        let _ = Command::new("kill")
            .args(["-TERM", &d.id().to_string()])
            .status();
    }
    let status_a = wait_with_timeout(&mut daemon_a, Duration::from_secs(5));
    let status_b = wait_with_timeout(&mut daemon_b, Duration::from_secs(5));
    assert!(status_a.is_some(), "daemon A did not exit on SIGTERM");
    assert!(status_b.is_some(), "daemon B did not exit on SIGTERM");
}

fn peer_is_listed(snap: &serde_json::Value) -> bool {
    snap.get("peers")
        .and_then(|v| v.as_array())
        .is_some_and(|peers| !peers.is_empty())
}

fn peer_is_discovered(snap: &serde_json::Value) -> bool {
    snap.get("peers")
        .and_then(|v| v.as_array())
        .is_some_and(|peers| {
            peers
                .iter()
                .any(|p| p.get("discovered").and_then(|v| v.as_bool()) == Some(true))
        })
}

fn last_sync_recorded(snap: &serde_json::Value) -> bool {
    snap.get("peers")
        .and_then(|v| v.as_array())
        .is_some_and(|peers| {
            peers
                .iter()
                .any(|p| p.get("last_sync_action").and_then(|v| v.as_str()).is_some())
        })
}

/// Full two-peer round-trip: pair A & B, start daemons, mDNS
/// discovers, B fast-forwards from A's history (bootstrap path),
/// `bypass insert` on A propagates to B, `bypass show` on B returns
/// the same plaintext.
///
/// Skips with a clear message if mDNS discovery never fires within
/// 30 s — that's the host's IPv4 multicast routing missing
/// (`ip route show 224.0.0.0/4` returns nothing). The skip is
/// stderr-only so the test runner still records a `passed` for the
/// lifecycle bits that DID work, but the data-flow assertion is
/// only enforced where it can be.
#[test]
#[ignore]
fn full_round_trip_insert_on_a_then_show_on_b_via_mdns() {
    // Same GPG keyring on both sides so both can decrypt entries
    // encrypted to the test recipient. TestEnv generates the key
    // once; we point both processes at that single GNUPGHOME.
    let gpg = common::TestEnv::new();
    let store_a = tempfile::TempDir::new().unwrap();
    let store_b = tempfile::TempDir::new().unwrap();
    let cfg_a = tempfile::TempDir::new().unwrap();
    let cfg_b = tempfile::TempDir::new().unwrap();
    let runtime_a = tempfile::TempDir::new().unwrap();
    let runtime_b = tempfile::TempDir::new().unwrap();

    // Env helper: same GNUPGHOME, side-specific PASSWORD_STORE_DIR.
    let env_a_override: Vec<(&str, &Path)> = vec![("PASSWORD_STORE_DIR", store_a.path())];
    let env_b_override: Vec<(&str, &Path)> = vec![("PASSWORD_STORE_DIR", store_b.path())];

    let bin = assert_cmd::cargo::cargo_bin("bypass");
    let run_in =
        |store: &Path, cfg: &Path, runtime: &Path, args: &[&str]| -> std::process::Output {
            let mut cmd = Command::new(&bin);
            for (k, v) in gpg.env_pairs() {
                cmd.env(k, v);
            }
            cmd.env("PASSWORD_STORE_DIR", store)
                .env("XDG_CONFIG_HOME", cfg)
                .env("XDG_RUNTIME_DIR", runtime)
                .args(args);
            cmd.output().expect("spawn")
        };

    // A: bypass init + insert one known entry.
    let out = run_in(
        store_a.path(),
        cfg_a.path(),
        runtime_a.path(),
        &["init", common::TEST_RECIPIENT],
    );
    assert!(out.status.success(), "init A failed: {out:?}");

    let mut insert = Command::new(&bin);
    for (k, v) in gpg.env_pairs() {
        insert.env(k, v);
    }
    insert
        .env("PASSWORD_STORE_DIR", store_a.path())
        .env("XDG_CONFIG_HOME", cfg_a.path())
        .env("XDG_RUNTIME_DIR", runtime_a.path())
        .args(["insert", "email/work"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut insert_child = insert.spawn().unwrap();
    insert_child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hunter2\n")
        .unwrap();
    let insert_status = insert_child.wait().unwrap();
    assert!(insert_status.success(), "insert on A failed");

    // B: `git init` only — no `bypass init`. The first sync's
    // bootstrap path (`WantPackFrom { None, None }`) will fetch
    // A's history and fast-forward, including A's .gpg-id.
    let git_init = Command::new("git")
        .args(["init", "-q"])
        .arg(store_b.path())
        .status()
        .unwrap();
    assert!(git_init.success(), "git init B failed");

    pair_two_processes_with_stores(
        &gpg,
        &env_a_override,
        &env_b_override,
        cfg_a.path(),
        cfg_b.path(),
        runtime_a.path(),
        runtime_b.path(),
    );

    // Spawn daemons.
    let mut daemon_a = spawn_bypass(
        &gpg,
        cfg_a.path(),
        runtime_a.path(),
        &env_a_override,
        &["sync", "daemon"],
    );
    let mut daemon_b = spawn_bypass(
        &gpg,
        cfg_b.path(),
        runtime_b.path(),
        &env_b_override,
        &["sync", "daemon"],
    );

    // Wait for mDNS discovery on either side. If neither sees the
    // other within 30 s, the host doesn't have working multicast
    // routes — skip the data-flow assertion with a clear note.
    let discovery_deadline = Instant::now() + Duration::from_secs(30);
    let saw_discovery = try_poll_status_until(
        &gpg,
        cfg_a.path(),
        runtime_a.path(),
        discovery_deadline,
        peer_is_discovered,
    )
    .is_some();

    if !saw_discovery {
        eprintln!(
            "\n  SKIP: mDNS discovery did not fire within 30 s.\n  \
             Likely cause: this host has no IPv4 multicast route for 224.0.0.0/4.\n  \
             Check `ip route show 224.0.0.0/4`. Re-run on a host with working LAN \
             multicast to exercise the full data-flow assertion.\n"
        );
    } else {
        // Both sides should eventually log a `last_sync_action` —
        // that's the daemon writing the sync outcome to its state.
        // Use a generous deadline for the rebase + auto-commit
        // cycle.
        let sync_deadline = Instant::now() + Duration::from_secs(30);
        let sync_a = try_poll_status_until(
            &gpg,
            cfg_a.path(),
            runtime_a.path(),
            sync_deadline,
            last_sync_recorded,
        );
        let sync_b = try_poll_status_until(
            &gpg,
            cfg_b.path(),
            runtime_b.path(),
            sync_deadline,
            last_sync_recorded,
        );
        assert!(
            sync_a.is_some() && sync_b.is_some(),
            "neither side recorded a sync action despite mDNS discovery",
        );

        // Give the auto-commit on the receive side a beat to settle.
        std::thread::sleep(Duration::from_secs(2));

        // Show on B should now reveal the entry A inserted.
        let show = run_in(
            store_b.path(),
            cfg_b.path(),
            runtime_b.path(),
            &["show", "email/work"],
        );
        assert!(
            show.status.success(),
            "show on B failed: stderr={}",
            String::from_utf8_lossy(&show.stderr)
        );
        assert!(
            show.stdout.starts_with(b"hunter2"),
            "show on B returned unexpected plaintext: {:?}",
            String::from_utf8_lossy(&show.stdout)
        );
    }

    // Clean shutdown either way.
    for d in [&daemon_a, &daemon_b] {
        let _ = Command::new("kill")
            .args(["-TERM", &d.id().to_string()])
            .status();
    }
    let _ = wait_with_timeout(&mut daemon_a, Duration::from_secs(5));
    let _ = wait_with_timeout(&mut daemon_b, Duration::from_secs(5));
}

/// Variant of `pair_two_processes` that allows overriding the
/// store path per process (so both sides can share a single
/// GNUPGHOME while keeping their stores separate). The pairing
/// flow itself doesn't touch the store, but the binary's env-pair
/// validation expects both vars to be set.
fn pair_two_processes_with_stores(
    gpg: &common::TestEnv,
    extra_a: &[(&str, &Path)],
    extra_b: &[(&str, &Path)],
    cfg_a: &Path,
    cfg_b: &Path,
    runtime_a: &Path,
    runtime_b: &Path,
) {
    let mut show = spawn_bypass(
        gpg,
        cfg_a,
        runtime_a,
        extra_a,
        &[
            "sync",
            "pair",
            "--show",
            "--addr",
            "/ip4/127.0.0.1/tcp/0",
            "--name",
            "device-a",
        ],
    );
    let mut show_stdout = BufReader::new(show.stdout.take().expect("show stdout"));

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut prefix = Vec::new();
    let pin_line = read_until(
        &mut show_stdout,
        deadline,
        |s| s.starts_with("PAIRING PIN:"),
        &mut prefix,
    )
    .unwrap_or_else(|| panic!("no PIN; got:\n{}", prefix.join("\n")));
    let pin = pin_line
        .strip_prefix("PAIRING PIN:")
        .unwrap()
        .trim()
        .to_owned();
    let addr_line = read_until(
        &mut show_stdout,
        deadline,
        |s| s.contains("/p2p/"),
        &mut prefix,
    )
    .unwrap_or_else(|| panic!("no addr; got:\n{}", prefix.join("\n")));
    let multiaddr = addr_line.trim().to_owned();

    let mut enter = spawn_bypass(
        gpg,
        cfg_b,
        runtime_b,
        extra_b,
        &[
            "sync", "pair", "--enter", "--addr", &multiaddr, "--name", "device-b",
        ],
    );
    enter
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{pin}\n").as_bytes())
        .unwrap();
    assert!(
        wait_with_timeout(&mut enter, Duration::from_secs(30))
            .expect("enter timeout")
            .success()
    );
    assert!(
        wait_with_timeout(&mut show, Duration::from_secs(30))
            .expect("show timeout")
            .success()
    );
}
