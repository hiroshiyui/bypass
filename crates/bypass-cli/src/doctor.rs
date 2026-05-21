// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass doctor`: read-only health probe of the user's environment.
//!
//! Each check prints one line `name   [status]   detail` and contributes
//! to a final exit code (1 if any check failed; warnings do not fail).
//! Nothing in this module mutates user state.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::storage_fs::StorageFs;

#[derive(Debug, Copy, Clone)]
enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    fn tag(self) -> &'static str {
        match self {
            Self::Ok => "[ok]  ",
            Self::Warn => "[warn]",
            Self::Fail => "[fail]",
        }
    }
}

struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn ok(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Ok,
            detail: detail.into(),
        }
    }
    fn warn(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Warn,
            detail: detail.into(),
        }
    }
    fn fail(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            status: Status::Fail,
            detail: detail.into(),
        }
    }
}

/// Run all checks, print the report, and return a process exit code
/// (0 if no checks failed, 1 otherwise).
pub fn run() -> i32 {
    let mut checks: Vec<Check> = Vec::new();

    let gpg_ok = run_check_gpg(&mut checks);
    if gpg_ok {
        run_check_secret_keys(&mut checks);
    }

    let root_result = StorageFs::resolve_default_root();
    let root = run_check_store_root(&mut checks, root_result);

    let recipients = match &root {
        Some(r) => run_check_gpg_id(&mut checks, r),
        None => None,
    };
    if gpg_ok && let Some(recipients) = recipients {
        run_check_recipients_known(&mut checks, &recipients);
    }

    run_check_editor(&mut checks);
    run_check_git(&mut checks);
    if let Some(r) = &root {
        run_check_gitattributes(&mut checks, r);
        run_check_leak_audit(&mut checks, r);
    }

    print_report(&checks);

    if checks.iter().any(|c| matches!(c.status, Status::Fail)) {
        1
    } else {
        0
    }
}

fn run_check_gpg(checks: &mut Vec<Check>) -> bool {
    match Command::new("gpg").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let first = String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .unwrap_or("")
                .to_owned();
            checks.push(Check::ok("gpg", first));
            true
        }
        Ok(out) => {
            checks.push(Check::fail(
                "gpg",
                format!("`gpg --version` exited with status {}", out.status),
            ));
            false
        }
        Err(e) => {
            checks.push(Check::fail("gpg", format!("cannot spawn `gpg`: {e}")));
            false
        }
    }
}

fn run_check_secret_keys(checks: &mut Vec<Check>) {
    match Command::new("gpg")
        .arg("--list-secret-keys")
        .arg("--with-colons")
        .output()
    {
        Ok(out) if out.status.success() => {
            let count = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|l| l.starts_with("sec:"))
                .count();
            if count == 0 {
                checks.push(Check::warn(
                    "secret keys",
                    "no secret keys in keyring; you will not be able to decrypt entries",
                ));
            } else {
                checks.push(Check::ok(
                    "secret keys",
                    format!("{count} secret key(s) available"),
                ));
            }
        }
        Ok(out) => checks.push(Check::warn(
            "secret keys",
            format!("`gpg --list-secret-keys` failed: status {}", out.status),
        )),
        Err(e) => checks.push(Check::warn("secret keys", format!("cannot spawn gpg: {e}"))),
    }
}

fn run_check_store_root(
    checks: &mut Vec<Check>,
    result: Result<PathBuf, crate::storage_fs::StorageFsError>,
) -> Option<PathBuf> {
    match result {
        Ok(root) => {
            if !root.exists() {
                checks.push(Check::warn(
                    "store root",
                    format!("{} does not exist (run `bypass init`)", root.display()),
                ));
                Some(root)
            } else if !root.is_dir() {
                checks.push(Check::fail(
                    "store root",
                    format!("{} exists but is not a directory", root.display()),
                ));
                None
            } else {
                checks.push(Check::ok("store root", root.display().to_string()));
                Some(root)
            }
        }
        Err(e) => {
            checks.push(Check::fail("store root", e.to_string()));
            None
        }
    }
}

fn run_check_gpg_id(checks: &mut Vec<Check>, root: &Path) -> Option<Vec<String>> {
    let gpg_id = root.join(".gpg-id");
    if !gpg_id.exists() {
        checks.push(Check::fail(
            ".gpg-id",
            format!(
                "{} does not exist; run `bypass init <gpg-id>` first",
                gpg_id.display()
            ),
        ));
        return None;
    }
    let bytes = match std::fs::read(&gpg_id) {
        Ok(b) => b,
        Err(e) => {
            checks.push(Check::fail(
                ".gpg-id",
                format!("cannot read {}: {e}", gpg_id.display()),
            ));
            return None;
        }
    };
    let text = match std::str::from_utf8(&bytes) {
        Ok(t) => t,
        Err(_) => {
            checks.push(Check::fail(".gpg-id", "file is not valid UTF-8"));
            return None;
        }
    };
    let recipients: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect();
    if recipients.is_empty() {
        checks.push(Check::fail(".gpg-id", "no recipients listed"));
        return None;
    }
    checks.push(Check::ok(
        ".gpg-id",
        format!(
            "{} recipient(s): {}",
            recipients.len(),
            recipients.join(", ")
        ),
    ));
    Some(recipients)
}

fn run_check_recipients_known(checks: &mut Vec<Check>, recipients: &[String]) {
    let mut missing: Vec<&str> = Vec::new();
    for r in recipients {
        let ok = Command::new("gpg")
            .arg("--list-keys")
            .arg("--")
            .arg(r)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !ok {
            missing.push(r.as_str());
        }
    }
    if missing.is_empty() {
        checks.push(Check::ok("recipients", "all recipients found in keyring"));
    } else {
        checks.push(Check::fail(
            "recipients",
            format!("not in keyring: {}", missing.join(", ")),
        ));
    }
}

fn run_check_editor(checks: &mut Vec<Check>) {
    match std::env::var("EDITOR") {
        Ok(e) if !e.is_empty() => {
            checks.push(Check::ok("EDITOR", e));
        }
        _ => checks.push(Check::warn(
            "EDITOR",
            "not set; `bypass edit` will fall back to `vi`",
        )),
    }
}

fn run_check_git(checks: &mut Vec<Check>) {
    match Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let v = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            checks.push(Check::ok("git", v));
        }
        _ => checks.push(Check::warn(
            "git",
            "git binary not found; `bypass sync` will be unavailable",
        )),
    }
}

fn run_check_gitattributes(checks: &mut Vec<Check>, root: &Path) {
    let path = root.join(".gitattributes");
    if !path.exists() {
        checks.push(Check::fail(
            ".gitattributes",
            "missing; `bypass sync` will install `*.gpg binary` automatically on next run",
        ));
        return;
    }
    let body = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            checks.push(Check::warn(
                ".gitattributes",
                format!("cannot read {}: {e}", path.display()),
            ));
            return;
        }
    };
    let text = String::from_utf8_lossy(&body);
    let has_rule = text.lines().any(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return false;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            return false;
        };
        pattern == "*.gpg" && parts.any(|tok| tok == "binary")
    });
    if has_rule {
        checks.push(Check::ok(
            ".gitattributes",
            "carries `*.gpg binary` rule (line-ending normalisation disabled)",
        ));
    } else {
        checks.push(Check::fail(
            ".gitattributes",
            "missing `*.gpg binary` rule; cross-platform clones may corrupt ciphertext",
        ));
    }
}

fn run_check_leak_audit(checks: &mut Vec<Check>, root: &Path) {
    if !root.join(".git").exists() {
        // Audit only makes sense once init has run.
        return;
    }
    match crate::audit::audit_for_push(root) {
        Ok(issues) if issues.is_empty() => {
            checks.push(Check::ok(
                "audit",
                "no plaintext or unknown files in the pending push",
            ));
        }
        Ok(issues) => {
            let preview = issues
                .iter()
                .take(3)
                .map(|i| i.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let more = if issues.len() > 3 {
                format!(", … (+{} more)", issues.len() - 3)
            } else {
                String::new()
            };
            checks.push(Check::fail(
                "audit",
                format!(
                    "{} suspicious file(s): {preview}{more}; run `bypass audit`",
                    issues.len()
                ),
            ));
        }
        Err(e) => {
            checks.push(Check::warn("audit", format!("could not run audit: {e}")));
        }
    }
}

fn print_report(checks: &[Check]) {
    let width = checks.iter().map(|c| c.name.len()).max().unwrap_or(0);
    for c in checks {
        println!(
            "{:width$}  {}  {}",
            c.name,
            c.status.tag(),
            c.detail,
            width = width
        );
    }
}
