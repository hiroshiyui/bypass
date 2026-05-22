// SPDX-License-Identifier: GPL-3.0-or-later

//! Body of the hidden `bypass __merge-take-theirs` subcommand.
//!
//! Registered as the merge driver for `*.gpg` via `.gitattributes`
//! ([ADR-0011](../../../../doc/adr/0011-sync-semantics-hybrid.md)). git
//! invokes it during rebase / merge whenever two sides disagree on the
//! contents of an opaque `.gpg` blob. We always resolve by taking the
//! incoming version: a `.gpg` blob is a self-contained ciphertext and a
//! 3-way merge of ciphertext bytes has no semantic meaning.
//!
//! Driver signature (from `git`'s `gitattributes(5)`):
//!
//! ```text
//! merge.bypass-take-theirs.driver = bypass __merge-take-theirs %O %A %B %P %L
//! ```
//!
//! - `%O`: ancestor blob path (unused — we don't 3-way merge).
//! - `%A`: current ("ours") blob path. The driver writes the resolved
//!   content here. Must exit 0 on success.
//! - `%B`: other ("theirs") blob path.
//! - `%P`: pathname (for diagnostics).
//! - `%L`: conflict-marker-size (unused).

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Friendly name git uses to look the driver up in `.git/config`. Must
/// match the `merge=` attribute written into `.gitattributes`.
pub const DRIVER_NAME: &str = "bypass-take-theirs";

/// Register the merge driver in `<root>/.git/config`. Idempotent —
/// `git config` overwrites the existing value. Uses subprocess `git`
/// rather than the `git2` config API because libgit2's repo-local
/// config writes have historically been quirky.
pub fn register_in_git_config(root: &Path) -> Result<()> {
    if !root.join(".git").exists() {
        // No repo yet; nothing to register. Caller controls when to
        // call this — typically after `bypass init` or as part of
        // `bypass sync`'s lazy install.
        return Ok(());
    }
    set_config(
        root,
        &format!("merge.{DRIVER_NAME}.name"),
        "bypass: take-theirs for opaque .gpg blobs",
    )?;
    // Re-exec the running `bypass` binary as the driver. Using
    // `current_exe()` keeps the registration self-contained — the
    // installed binary's path is baked into `.git/config`, so a later
    // `git rebase` invocation finds the same `bypass` that wrote it.
    let me = std::env::current_exe().context("resolve current `bypass` binary path")?;
    let driver_cmd = format!(
        "{} __merge-take-theirs %O %A %B %P %L",
        shell_quote(&me.to_string_lossy())
    );
    set_config(root, &format!("merge.{DRIVER_NAME}.driver"), &driver_cmd)?;
    Ok(())
}

fn set_config(root: &Path, key: &str, value: &str) -> Result<()> {
    let status = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", key, value])
        .status()
        .with_context(|| format!("spawn `git config {key}`"))?;
    if !status.success() {
        bail!(
            "`git config {key}` failed (exit {})",
            status.code().unwrap_or(-1)
        );
    }
    Ok(())
}

/// Wrap a string in single quotes for safe `git config` storage, escaping
/// any embedded single quotes. `.git/config` parses values with shell-
/// like rules; quoting protects paths containing spaces.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Resolve the conflict by copying `theirs` over `ours`. Returns the
/// exit code to surface (always `0` on success; non-zero is via
/// `anyhow::Error` propagated by the caller).
pub fn take_theirs(_ancestor: &Path, ours: &Path, theirs: &Path, path: &str) -> Result<u8> {
    let bytes = fs::read(theirs)
        .with_context(|| format!("read theirs side of {path} from {}", theirs.display()))?;
    fs::write(ours, &bytes)
        .with_context(|| format!("write resolved {path} to {}", ours.display()))?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn take_theirs_overwrites_ours_with_theirs_bytes() {
        let td = TempDir::new().unwrap();
        let ancestor = td.path().join("O");
        let ours = td.path().join("A");
        let theirs = td.path().join("B");
        fs::write(&ancestor, b"old").unwrap();
        fs::write(&ours, b"ours-version").unwrap();
        fs::write(&theirs, b"theirs-version").unwrap();

        let exit = take_theirs(&ancestor, &ours, &theirs, "email/work.gpg").unwrap();
        assert_eq!(exit, 0);
        assert_eq!(fs::read(&ours).unwrap(), b"theirs-version");
        // Theirs is left intact; git owns its lifetime.
        assert_eq!(fs::read(&theirs).unwrap(), b"theirs-version");
    }

    #[test]
    fn take_theirs_errors_if_theirs_missing() {
        let td = TempDir::new().unwrap();
        let err = take_theirs(
            &td.path().join("O"),
            &td.path().join("A"),
            &td.path().join("B"),
            "x.gpg",
        )
        .unwrap_err();
        // Message mentions the path for debuggability.
        assert!(err.to_string().contains("theirs"), "got: {err:#}");
    }
}
