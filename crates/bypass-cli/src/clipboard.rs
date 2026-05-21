// SPDX-License-Identifier: GPL-3.0-or-later

//! Clipboard integration for `bypass show -c` and `bypass generate -c`.
//!
//! Architecture: the foreground process forks a detached helper child
//! (re-executing the same binary with a hidden `__clipboard-set`
//! subcommand). The child receives the password on stdin, takes
//! ownership of the system clipboard via `arboard`, sleeps for the
//! requested delay, and — if the clipboard still contains exactly the
//! password we wrote — restores whatever was there before. The
//! foreground process exits immediately so the user's shell prompt
//! returns.
//!
//! The hidden child is necessary because on X11 the clipboard contents
//! are tied to the client connection that wrote them; if the
//! foreground process exited while owning the clipboard, the X server
//! would drop them. Spinning up a fresh `arboard::Clipboard` inside the
//! child keeps the connection alive for the full auto-clear window.
//!
//! Restoration is conditional on the clipboard still matching the
//! password: if the user copied something else during the wait window,
//! we leave their new selection alone.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

/// Default auto-clear delay, in seconds. Matches `pass`'s default.
pub const DEFAULT_CLEAR_SECS: u64 = 45;

/// Internal-subcommand name used to re-invoke the daemon child. The
/// leading underscores discourage accidental user invocation and keep
/// this off the published `--help` (the clap variant is `hide=true`).
pub const DAEMON_SUBCOMMAND: &str = "__clipboard-set";

/// Spawn the detached clipboard daemon and return immediately. The
/// daemon takes ownership of the clipboard and restores it after
/// `seconds`. The caller does not block.
pub fn copy_and_auto_clear(password: &[u8], seconds: u64) -> Result<()> {
    let exe = std::env::current_exe().context("locate self exe")?;
    let mut child = Command::new(&exe)
        .arg(DAEMON_SUBCOMMAND)
        .arg(seconds.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // process_group(0) puts the daemon into its own group so it
        // survives the foreground process exiting. It does NOT detach
        // from the controlling terminal (a SIGHUP from closing the term
        // would still kill it), which is acceptable for a 45-second
        // helper.
        .process_group(0)
        .spawn()
        .with_context(|| format!("spawn clipboard daemon: {}", exe.display()))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .expect("stdin requested via Stdio::piped");
        stdin
            .write_all(password)
            .context("write password to clipboard daemon")?;
    }
    eprintln!("Copied to clipboard. Will clear in {seconds} seconds.");
    Ok(())
}

/// Daemon body. Invoked by re-exec from [`copy_and_auto_clear`]; do not
/// call directly from foreground code.
pub fn run_daemon(seconds: u64) -> Result<()> {
    // Read the password to install. Reading the entire stdin buffer
    // also serves as the synchronisation point: the parent has finished
    // writing it before we touch the clipboard.
    let mut password = Vec::new();
    std::io::stdin()
        .read_to_end(&mut password)
        .context("read password from stdin")?;
    if password.is_empty() {
        bail!("clipboard daemon received an empty password on stdin");
    }
    let password = String::from_utf8(password)
        .context("password is not valid UTF-8 (clipboards only accept text)")?;

    let mut cb = arboard::Clipboard::new().context("open system clipboard")?;
    // Snapshot whatever's in the clipboard right now so we can put it
    // back when our window closes.
    let previous = cb.get_text().ok();
    cb.set_text(&password)
        .context("write password to clipboard")?;

    thread::sleep(Duration::from_secs(seconds));

    let still_ours = cb.get_text().map(|t| t == password).unwrap_or(false);
    if still_ours {
        match previous {
            Some(prev) => {
                let _ = cb.set_text(prev);
            }
            None => {
                let _ = cb.clear();
            }
        }
    }
    Ok(())
}
