// SPDX-License-Identifier: GPL-3.0-or-later

//! Two-process loopback tests for sync.
//!
//! Marked `#[ignore]` by default per
//! [ADR-0013](../../../doc/adr/0013-sync-transport-trait.md): they
//! spin up two `bypass` child processes, do a real libp2p handshake
//! over `127.0.0.1`, and run for several seconds. The unit-test +
//! single-process loopback tier covers most regressions; this file
//! catches the multi-process integration ones.
//!
//! Run on demand:
//!
//! ```sh
//! cargo test -p bypass --test sync_loopback -- --ignored
//! ```

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

mod common;

/// Spawn `bypass <args>` with the same env-pair plumbing the other
/// integration tests use, plus a per-process `XDG_CONFIG_HOME`. Returns
/// the spawned child with piped stdin/stdout/stderr.
fn spawn_bypass(env: &common::TestEnv, cfg: &Path, args: &[&str]) -> Child {
    let exe = assert_cmd::cargo::cargo_bin("bypass");
    let mut cmd = Command::new(exe);
    for (k, v) in env.env_pairs() {
        cmd.env(k, v);
    }
    cmd.env("XDG_CONFIG_HOME", cfg)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.spawn().expect("spawn bypass child")
}

/// Read lines from a piped stdout until `pred` matches one (returning
/// the matching line) or `deadline` elapses. Earlier non-matching
/// lines are buffered and joined into `prefix` for diagnostics.
fn read_until(
    reader: &mut BufReader<ChildStdout>,
    deadline: Instant,
    mut pred: impl FnMut(&str) -> bool,
    prefix: &mut Vec<String>,
) -> Option<String> {
    while Instant::now() < deadline {
        let mut line = String::new();
        // BufRead::read_line blocks. We poll-by-byte instead so the
        // deadline is honoured. Simpler approach: leave the reader
        // blocking; the show-side prints the PIN line within
        // milliseconds of spawn, and if it doesn't the test should
        // fail anyway. Use a small thread to enforce the deadline.
        match reader.read_line(&mut line) {
            Ok(0) => return None, // EOF
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

fn peers_toml(cfg: &Path) -> PathBuf {
    cfg.join("bypass").join("peers.toml")
}

#[test]
#[ignore]
fn two_processes_pair_via_libp2p_and_pin_each_other_in_peers_toml() {
    // Each side gets its own `XDG_CONFIG_HOME` so the identity keys
    // and `peers.toml` files don't collide. Both share the same
    // throwaway GPG keyring + store dir — the pairing flow doesn't
    // touch either, but the env helper requires them.
    let env = common::TestEnv::new();
    let cfg_show = tempfile::TempDir::new().unwrap();
    let cfg_enter = tempfile::TempDir::new().unwrap();

    // Show side: listen on a random localhost port, print PIN + addr.
    let mut show = spawn_bypass(
        &env,
        cfg_show.path(),
        &[
            "sync",
            "pair",
            "--show",
            "--addr",
            "/ip4/127.0.0.1/tcp/0",
            "--name",
            "show-device",
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
    .unwrap_or_else(|| panic!("never saw PAIRING PIN line; got:\n{}", prefix.join("\n")));
    let pin = pin_line
        .strip_prefix("PAIRING PIN:")
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(pin.len(), 6, "PIN should be 6 digits, got {pin:?}");

    // The next non-banner line that contains `/p2p/` is the multiaddr
    // the enter side needs to dial.
    let addr_line = read_until(
        &mut show_stdout,
        deadline,
        |s| s.contains("/p2p/"),
        &mut prefix,
    )
    .unwrap_or_else(|| panic!("never saw a /p2p/ multiaddr; got:\n{}", prefix.join("\n")));
    let multiaddr = addr_line.trim().to_owned();

    // Enter side: pipe the PIN to stdin, dial the show-side multiaddr.
    let mut enter = spawn_bypass(
        &env,
        cfg_enter.path(),
        &[
            "sync",
            "pair",
            "--enter",
            "--addr",
            &multiaddr,
            "--name",
            "enter-device",
        ],
    );
    {
        let stdin = enter.stdin.as_mut().expect("enter stdin");
        stdin
            .write_all(format!("{pin}\n").as_bytes())
            .expect("write PIN");
    }

    // Wait for both to exit. Either side hanging past 30 s indicates a
    // libp2p handshake regression.
    let enter_status =
        wait_with_timeout(&mut enter, Duration::from_secs(30)).expect("enter side timed out");
    let show_status =
        wait_with_timeout(&mut show, Duration::from_secs(30)).expect("show side timed out");
    assert!(
        enter_status.success(),
        "enter side exited non-zero: {enter_status}"
    );
    assert!(
        show_status.success(),
        "show side exited non-zero: {show_status}"
    );

    // Both sides should now have a peers.toml with the other peer's
    // record. Identity keys are random per-process, so the only
    // assertion is "the other side's record is there".
    let show_peers =
        std::fs::read_to_string(peers_toml(cfg_show.path())).expect("show peers.toml exists");
    let enter_peers =
        std::fs::read_to_string(peers_toml(cfg_enter.path())).expect("enter peers.toml exists");
    assert!(
        show_peers.contains("enter-device"),
        "show peers.toml missing enter-device:\n{show_peers}"
    );
    assert!(
        enter_peers.contains("show-device"),
        "enter peers.toml missing show-device:\n{enter_peers}"
    );
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
