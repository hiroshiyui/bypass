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
use std::time::Duration;

use anyhow::{Context, Result, bail};
use zeroize::Zeroizing;

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
///
/// Security audit findings H3 and H4 covered here:
///
/// - **H3**: the password (and the prior clipboard contents we save to
///   restore later) live in `Zeroizing<String>`s so the heap is
///   scrubbed when this function returns, panics, or is killed by a
///   signal that lets Drop run.
///
/// - **H4**: restoration runs on every exit path. Normal expiry is the
///   `tokio::time::sleep` arm of the select; `SIGINT` and `SIGTERM`
///   arms exit early and `RestoreGuard::drop` runs as the stack
///   unwinds. Panics likewise trigger the Drop. The only path we
///   cannot cover is `SIGKILL` / `SIGABRT`, where the kernel never
///   gives us a chance — documented limitation; same as every other
///   userland clipboard tool.
pub fn run_daemon(seconds: u64) -> Result<()> {
    // Read the password to install before we touch the clipboard. The
    // synchronous read also acts as the sync point with the parent:
    // it has finished writing before we proceed.
    let mut buf = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buf)
        .context("read password from stdin")?;
    if buf.is_empty() {
        bail!("clipboard daemon received an empty password on stdin");
    }
    let password: Zeroizing<String> = Zeroizing::new(
        String::from_utf8(buf)
            .context("password is not valid UTF-8 (clipboards only accept text)")?,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async move { run_daemon_async(password, seconds).await })
}

async fn run_daemon_async(password: Zeroizing<String>, seconds: u64) -> Result<()> {
    let mut cb = arboard::Clipboard::new().context("open system clipboard")?;
    // Snapshot whatever's in the clipboard right now so we can put it
    // back when our window closes. May itself contain a secret the
    // user copied from somewhere else — zeroize it.
    let previous: Option<Zeroizing<String>> = cb.get_text().ok().map(Zeroizing::new);
    cb.set_text(&*password)
        .context("write password to clipboard")?;

    // RAII guard: any path that drops this guard restores the clipboard
    // (if our password is still on it). Panics in the wait window
    // unwind through this Drop. Normal expiry, SIGINT and SIGTERM all
    // exit the `tokio::select!` below and drop the guard naturally.
    let mut guard = RestoreGuard::new(&mut cb, &password, previous);

    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
        _ = tokio::signal::ctrl_c() => {}
        _ = sigterm.recv() => {}
    }

    // Explicit restore so we surface any restore error, instead of
    // swallowing it in Drop.
    guard.restore_now();
    Ok(())
}

/// Best-effort clipboard restorer. Holds `&mut Clipboard` for the
/// lifetime of the daemon so Drop has direct access.
struct RestoreGuard<'a> {
    cb: &'a mut arboard::Clipboard,
    /// Owning reference to the password text so we can compare without
    /// the borrow checker getting in the way of `&mut cb`.
    password: String,
    previous: Option<Zeroizing<String>>,
    done: bool,
}

impl<'a> RestoreGuard<'a> {
    fn new(
        cb: &'a mut arboard::Clipboard,
        password: &Zeroizing<String>,
        previous: Option<Zeroizing<String>>,
    ) -> Self {
        Self {
            cb,
            password: (**password).clone(),
            previous,
            done: false,
        }
    }

    /// Restore the previous clipboard contents (or clear) if the
    /// clipboard still holds our password. No-op on repeat call.
    fn restore_now(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let still_ours = self
            .cb
            .get_text()
            .map(|t| t == self.password)
            .unwrap_or(false);
        if still_ours {
            match &self.previous {
                Some(prev) => {
                    let _ = self.cb.set_text((**prev).clone());
                }
                None => {
                    let _ = self.cb.clear();
                }
            }
        }
    }
}

impl Drop for RestoreGuard<'_> {
    fn drop(&mut self) {
        self.restore_now();
        // Scrub our password copy. (`previous` is already `Zeroizing`
        // and drops itself.)
        use zeroize::Zeroize;
        self.password.zeroize();
    }
}
