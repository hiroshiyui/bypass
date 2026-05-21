// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass ext <name> [args…]`: discover and execute pass-compatible
//! extensions.
//!
//! Discovery order (highest priority first), matching `pass`:
//!
//! 1. `<store-root>/.extensions/<name>` — extensions that travel with
//!    the store (i.e. live in the git repo).
//! 2. `$PASSWORD_STORE_EXTENSIONS_DIR/<name>` if the env var is set.
//! 3. `~/.password-store-extensions/<name>` — the conventional
//!    user-level directory.
//!
//! A candidate must be a regular file with at least one execute bit
//! set. If multiple candidates match, the first one in the list above
//! wins.
//!
//! Environment passed to the extension:
//!
//! - `PASSWORD_STORE_DIR` — the resolved store root.
//! - `PASSWORD_STORE_BIN` — the absolute path of the `bypass` binary,
//!   so extensions can call back into it (e.g. `"$PASSWORD_STORE_BIN" show foo`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use crate::storage_fs::StorageFs;

pub fn dispatch(name: &str, args: &[String]) -> Result<u8> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name == ".." {
        return Err(anyhow!("invalid extension name: {name:?}"));
    }

    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let candidates = candidate_paths(name, &root);

    let exe = candidates
        .iter()
        .find(|p| is_executable_file(p))
        .ok_or_else(|| {
            let tried = candidates
                .iter()
                .map(|p| format!("\n  - {}", p.display()))
                .collect::<String>();
            anyhow!("extension `{name}` not found (tried:{tried}\n)")
        })?
        .clone();

    let bypass_bin = std::env::current_exe().context("locate self exe")?;
    let status = Command::new(&exe)
        .args(args)
        .env("PASSWORD_STORE_DIR", &root)
        .env("PASSWORD_STORE_BIN", &bypass_bin)
        .status()
        .with_context(|| format!("spawn extension {}", exe.display()))?;
    Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
}

fn candidate_paths(name: &str, store_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();

    // 1. Store-local.
    out.push(store_root.join(".extensions").join(name));

    // 2. PASSWORD_STORE_EXTENSIONS_DIR env override.
    if let Ok(dir) = std::env::var("PASSWORD_STORE_EXTENSIONS_DIR")
        && !dir.is_empty()
    {
        out.push(PathBuf::from(dir).join(name));
    }

    // 3. ~/.password-store-extensions/<name>.
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".password-store-extensions").join(name));
    }

    out
}

#[cfg(unix)]
fn is_executable_file(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn candidate_paths_include_store_local_and_home() {
        let td = TempDir::new().unwrap();
        let paths = candidate_paths("foo", td.path());
        assert!(paths[0].ends_with(".extensions/foo"));
        // At least the store-local candidate must be present; the env-
        // var and home candidates are environment-dependent so we just
        // assert the count is reasonable.
        assert!(!paths.is_empty());
    }

    #[test]
    fn invalid_extension_names_are_rejected() {
        // We can't easily exercise the spawn path from a unit test (it
        // depends on $HOME and $PASSWORD_STORE_DIR), but the name
        // validation runs first.
        for bad in ["", "../etc/passwd", "a/b", r"a\b", ".."] {
            let err = dispatch(bad, &[]).unwrap_err();
            assert!(
                err.to_string().contains("invalid extension name"),
                "expected rejection for {bad:?}, got {err}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn is_executable_file_respects_x_bit() {
        use std::os::unix::fs::PermissionsExt;
        let td = TempDir::new().unwrap();
        let p = td.path().join("script");
        std::fs::write(&p, b"#!/bin/sh\necho hi\n").unwrap();

        // No exec bit yet.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_file(&p));

        // Owner exec bit set.
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&p));

        // Directory with x bit is *not* an executable file.
        let d = td.path().join("dir");
        std::fs::create_dir(&d).unwrap();
        assert!(!is_executable_file(&d));
    }
}
