// SPDX-License-Identifier: GPL-3.0-or-later

//! Sync-daemon supervisor integration: systemd user unit on Linux,
//! launchd user agent on macOS. See
//! [ADR-0020](../../../../doc/adr/0020-daemon-service-supervision.md).
//!
//! Unit / plist templates have the running binary's absolute path
//! baked in at `install` time (`std::env::current_exe()`), so a later
//! `bypass` upgrade that changes the install path requires
//! re-running `bypass sync daemon install` — mirrored in the
//! function's docstring.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const LAUNCHD_LABEL: &str = "io.bypass.sync";

// ----- public surface --------------------------------------------------

/// Write the systemd user unit or launchd user-agent plist to the
/// conventional path. Idempotent — overwrites any existing
/// `bypass`-owned file. Runs `systemctl --user daemon-reload` on
/// Linux so the new unit is picked up.
pub fn install() -> Result<u8> {
    let exe = current_exe()?;
    let path = unit_path()?;
    let body = render_unit(&exe);
    write_unit_file(&path, &body)?;
    reload_supervisor()?;
    eprintln!("bypass-sync: wrote {}", path.display());
    eprintln!(
        "bypass-sync: enable autostart with `bypass sync daemon enable`, \
         or run once with `bypass sync daemon start`"
    );
    Ok(0)
}

/// Remove the systemd / launchd file installed by [`install`]. Best
/// effort: if the file isn't there, says so but exits 0. Always runs
/// `daemon-reload` on Linux afterwards.
pub fn uninstall() -> Result<u8> {
    let path = unit_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => eprintln!("bypass-sync: removed {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            eprintln!(
                "bypass-sync: {} was not present; nothing to remove",
                path.display()
            );
        }
        Err(source) => {
            return Err(anyhow::Error::from(source).context(format!("remove {}", path.display())));
        }
    }
    reload_supervisor()?;
    Ok(0)
}

/// Ask the platform supervisor to start the daemon now (not boot-
/// persistent unless `enable` was also run).
pub fn start() -> Result<u8> {
    supervisor_op(SupervisorOp::Start)
}

pub fn stop() -> Result<u8> {
    supervisor_op(SupervisorOp::Stop)
}

pub fn enable() -> Result<u8> {
    supervisor_op(SupervisorOp::Enable)
}

pub fn disable() -> Result<u8> {
    supervisor_op(SupervisorOp::Disable)
}

/// Surface the supervisor's view of the daemon. Distinct from
/// [`super::socket::query_status`] — see ADR-0020:
///
/// - `bypass sync daemon status`: is the supervisor running it?
/// - `bypass sync status`: what does the running daemon see?
pub fn status() -> Result<u8> {
    supervisor_op(SupervisorOp::Status)
}

// ----- supervisor calls -----------------------------------------------

enum SupervisorOp {
    Start,
    Stop,
    Enable,
    Disable,
    Status,
}

#[cfg(target_os = "linux")]
fn supervisor_op(op: SupervisorOp) -> Result<u8> {
    let verb = match op {
        SupervisorOp::Start => "start",
        SupervisorOp::Stop => "stop",
        SupervisorOp::Enable => "enable",
        SupervisorOp::Disable => "disable",
        SupervisorOp::Status => "status",
    };
    let status = Command::new("systemctl")
        .args(["--user", verb, "bypass-sync.service"])
        .status()
        .context("spawn `systemctl --user`")?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

#[cfg(target_os = "macos")]
fn supervisor_op(op: SupervisorOp) -> Result<u8> {
    let uid = current_uid();
    let target = format!("gui/{uid}/{LAUNCHD_LABEL}");
    let status = match op {
        SupervisorOp::Start => Command::new("launchctl")
            .args(["kickstart", &target])
            .status(),
        SupervisorOp::Stop => Command::new("launchctl")
            .args(["bootout", &target])
            .status(),
        SupervisorOp::Enable => {
            // Flip RunAtLoad to true so it starts at login.
            let path = unit_path()?;
            set_run_at_load(&path, true)?;
            // Bootstrap so the change takes effect this session too.
            Command::new("launchctl")
                .args(["bootstrap", &format!("gui/{uid}"), &path.to_string_lossy()])
                .status()
        }
        SupervisorOp::Disable => {
            let path = unit_path()?;
            set_run_at_load(&path, false)?;
            Command::new("launchctl")
                .args(["bootout", &target])
                .status()
        }
        SupervisorOp::Status => Command::new("launchctl").args(["print", &target]).status(),
    }
    .context("spawn `launchctl`")?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

// Other Unixes (e.g. *BSD) — explicitly unsupported for v1; the
// daemon itself is gated on `unix` so they'd already fail elsewhere,
// but be specific about the supervisor.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn supervisor_op(_op: SupervisorOp) -> Result<u8> {
    bail!(
        "service supervision is supported on Linux (systemd) and macOS \
         (launchd) only; run `bypass sync daemon` in the foreground \
         instead, or open an issue if you'd like another supervisor"
    );
}

#[cfg(target_os = "linux")]
fn reload_supervisor() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
        .context("spawn `systemctl --user daemon-reload`")?;
    if !status.success() {
        bail!(
            "`systemctl --user daemon-reload` failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn reload_supervisor() -> Result<()> {
    // launchd picks up the new plist on the next `bootstrap`; there's
    // no equivalent of `daemon-reload`. No-op.
    Ok(())
}

// ----- path / template helpers -----------------------------------------

fn unit_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot resolve $HOME; set the variable manually")?;
    #[cfg(target_os = "linux")]
    {
        Ok(home.join(".config/systemd/user/bypass-sync.service"))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(home.join("Library/LaunchAgents/io.bypass.sync.plist"))
    }
    #[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
    {
        let _ = home;
        bail!(
            "service supervision is supported on Linux (systemd) and macOS \
             (launchd) only"
        )
    }
}

fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("resolve current `bypass` binary path for service install")
}

fn write_unit_file(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create directory {}", parent.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn current_uid() -> u32 {
    // Mirrors `sync::socket::current_uid`. Local copy to avoid
    // cross-module visibility expansion just for one syscall.
    unsafe extern "C" {
        #[link_name = "getuid"]
        fn libc_getuid() -> u32;
    }
    unsafe { libc_getuid() }
}

#[cfg(target_os = "macos")]
fn set_run_at_load(path: &Path, value: bool) -> Result<()> {
    // Quick-and-dirty: read the file, swap the RunAtLoad bool, write
    // it back. The plist is generated by us so we know its exact
    // shape — no need for a full XML parser.
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let new = if value {
        text.replace(
            "<key>RunAtLoad</key>\n      <false/>",
            "<key>RunAtLoad</key>\n      <true/>",
        )
    } else {
        text.replace(
            "<key>RunAtLoad</key>\n      <true/>",
            "<key>RunAtLoad</key>\n      <false/>",
        )
    };
    std::fs::write(path, new).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

// ----- unit / plist templates ------------------------------------------

#[cfg(target_os = "linux")]
fn render_unit(exe: &Path) -> String {
    format!(
        "# SPDX-License-Identifier: GPL-3.0-or-later\n\
         # Auto-generated by `bypass sync daemon install` (ADR-0020).\n\
         # Re-run that command after upgrading `bypass` so the path\n\
         # below stays in sync with the actual binary.\n\
         [Unit]\n\
         Description=bypass-sync LAN peer-to-peer sync daemon\n\
         Documentation=https://github.com/hiroshiyui/bypass\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} sync daemon\n\
         Restart=on-failure\n\
         RestartSec=10\n\
         Environment=RUST_LOG=info\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        exe = exe.display(),
    )
}

#[cfg(target_os = "macos")]
fn render_unit(exe: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!-- SPDX-License-Identifier: GPL-3.0-or-later -->\n\
         <!-- Auto-generated by `bypass sync daemon install` (ADR-0020).\n\
              Re-run that command after upgrading `bypass`. -->\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20\x20<key>Label</key>\n\
         \x20\x20<string>{LAUNCHD_LABEL}</string>\n\
         \x20\x20<key>ProgramArguments</key>\n\
         \x20\x20<array>\n\
         \x20\x20\x20\x20<string>{exe}</string>\n\
         \x20\x20\x20\x20<string>sync</string>\n\
         \x20\x20\x20\x20<string>daemon</string>\n\
         \x20\x20</array>\n\
         \x20\x20<key>RunAtLoad</key>\n\
         \x20\x20\x20\x20\x20\x20<false/>\n\
         \x20\x20<key>KeepAlive</key>\n\
         \x20\x20<dict>\n\
         \x20\x20\x20\x20<key>SuccessfulExit</key>\n\
         \x20\x20\x20\x20<false/>\n\
         \x20\x20</dict>\n\
         \x20\x20<key>StandardOutPath</key>\n\
         \x20\x20<string>/tmp/bypass-sync.log</string>\n\
         \x20\x20<key>StandardErrorPath</key>\n\
         \x20\x20<string>/tmp/bypass-sync.log</string>\n\
         </dict>\n\
         </plist>\n",
        exe = exe.display(),
    )
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn render_unit(_exe: &Path) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn rendered_systemd_unit_has_bin_path_and_restart_directive() {
        let body = render_unit(Path::new("/opt/bypass/bin/bypass"));
        assert!(body.contains("ExecStart=/opt/bypass/bin/bypass sync daemon"));
        assert!(body.contains("Restart=on-failure"));
        assert!(body.contains("[Install]\nWantedBy=default.target"));
        assert!(body.starts_with("# SPDX-License-Identifier:"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn rendered_launchd_plist_has_program_arguments_and_label() {
        let body = render_unit(Path::new("/usr/local/bin/bypass"));
        assert!(body.contains("<string>io.bypass.sync</string>"));
        assert!(body.contains("<string>/usr/local/bin/bypass</string>"));
        assert!(body.contains("<key>KeepAlive</key>"));
        assert!(body.contains("<string>sync</string>"));
        assert!(body.contains("<string>daemon</string>"));
        // RunAtLoad ships off; the user opts in via `enable`.
        assert!(body.contains("<key>RunAtLoad</key>\n      <false/>"));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unit_path_lives_under_xdg_systemd_user() {
        // We can't assert the exact path without HOME, but the suffix
        // should match the ADR-0020 conventional location.
        let p = unit_path().unwrap();
        assert!(
            p.ends_with(".config/systemd/user/bypass-sync.service"),
            "got {}",
            p.display()
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn unit_path_lives_under_library_launchagents() {
        let p = unit_path().unwrap();
        assert!(
            p.ends_with("Library/LaunchAgents/io.bypass.sync.plist"),
            "got {}",
            p.display()
        );
    }
}
