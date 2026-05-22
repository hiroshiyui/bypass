// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem-backed [`Storage`] for the Android (or any
//! single-tenant app-sandbox) target. Per
//! [ADR-0024](../../../../doc/adr/0024-android-ffi-via-uniffi.md)
//! §"Storage on Android" this is intentionally slimmer than the
//! desktop CLI's
//! [`StorageFs`](../../../../crates/bypass-cli/src/storage_fs.rs):
//!
//! - **No shred-on-remove.** Android wipes the app's private
//!   `filesDir` on uninstall and isolates it from other apps;
//!   `remove` just unlinks. The desktop ADR-0008 shred posture is
//!   for shared-host machines.
//! - **No symlink rejection.** App-private dirs aren't writable
//!   by another process so symlinks can't be planted (Phase 6's
//!   `F1` remediation isn't applicable here).
//! - **Atomic write (tempfile + rename) IS kept.** A crash mid-
//!   write must not leave a half-written ciphertext blob — same
//!   correctness story as the CLI.
//! - **Mode 0600 on tempfile create** via `OpenOptionsExt::mode`,
//!   same atomic-mode posture as the CLI's Phase 6 `E1`
//!   hardening.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use bypass_core::path::RelPath;
use bypass_core::storage::Storage;

#[derive(Debug, thiserror::Error)]
pub enum AppStorageError {
    #[error("I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("path resolved outside store root: {0}")]
    OutsideRoot(PathBuf),
}

fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> AppStorageError {
    AppStorageError::Io {
        path: path.into(),
        source,
    }
}

#[derive(Debug, Clone)]
pub struct AppStorage {
    root: PathBuf,
}

impl AppStorage {
    /// Construct rooted at `root`. The directory must already
    /// exist or be creatable on first write (Android's
    /// `filesDir` always exists).
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn absolute(&self, rel: &RelPath) -> Result<PathBuf, AppStorageError> {
        let abs = self.root.join(rel.as_str());
        // Defence-in-depth — `RelPath` already forbids `..` and
        // absolute paths.
        if !abs.starts_with(&self.root) {
            return Err(AppStorageError::OutsideRoot(abs));
        }
        Ok(abs)
    }
}

impl Storage for AppStorage {
    type Error = AppStorageError;

    fn read(&self, path: &RelPath) -> Result<Option<Vec<u8>>, Self::Error> {
        let abs = self.absolute(path)?;
        match fs::read(&abs) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(io_err(abs, e)),
        }
    }

    fn write(&mut self, path: &RelPath, data: &[u8]) -> Result<(), Self::Error> {
        let abs = self.absolute(path)?;
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err(parent.to_owned(), e))?;
        }
        // Atomic write: tempfile in destination dir, then rename.
        let parent = abs.parent().unwrap_or_else(|| Path::new("."));
        let leaf = abs.file_name().unwrap_or_default();
        let tmp = unique_tmp_path(parent, leaf);
        // Atomic create-with-mode: pass 0o600 to open(2) so the file
        // never exists at umask-default-mode (mirrors the CLI's
        // Phase 6 `E1` fix in `crates/bypass-cli/src/storage_fs.rs`).
        #[cfg(unix)]
        let mut tmp_file = {
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| io_err(tmp.clone(), e))?
        };
        #[cfg(not(unix))]
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io_err(tmp.clone(), e))?;

        let write_res = tmp_file.write_all(data).and_then(|()| tmp_file.sync_all());
        drop(tmp_file);
        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp);
            return Err(io_err(tmp, e));
        }
        if let Err(e) = fs::rename(&tmp, &abs) {
            let _ = fs::remove_file(&tmp);
            return Err(io_err(abs, e));
        }
        Ok(())
    }

    fn remove(&mut self, path: &RelPath) -> Result<(), Self::Error> {
        let abs = self.absolute(path)?;
        match fs::remove_file(&abs) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(abs, e)),
        }
    }

    fn exists(&self, path: &RelPath) -> Result<bool, Self::Error> {
        Ok(self.absolute(path)?.exists())
    }

    fn list(&self, prefix: Option<&RelPath>) -> Result<Vec<RelPath>, Self::Error> {
        let start = match prefix {
            Some(p) => self.absolute(p)?,
            None => self.root.clone(),
        };
        if !start.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        walk(&start, &self.root, &mut out)?;
        Ok(out)
    }
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<RelPath>) -> Result<(), AppStorageError> {
    let read = fs::read_dir(dir).map_err(|e| io_err(dir.to_owned(), e))?;
    for entry in read {
        let entry = entry.map_err(|e| io_err(dir.to_owned(), e))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|e| io_err(path.clone(), e))?;
        if file_type.is_dir() {
            walk(&path, root, out)?;
        } else if file_type.is_file()
            && let Ok(rel) = path.strip_prefix(root)
            && let Some(s) = rel.to_str()
            && let Ok(rp) = RelPath::new(s)
        {
            out.push(rp);
        }
    }
    Ok(())
}

fn unique_tmp_path(parent: &Path, leaf: &std::ffi::OsStr) -> PathBuf {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let leaf = leaf.to_string_lossy();
    parent.join(format!(".bypass.tmp.{pid}.{nanos}.{leaf}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn fresh() -> (TempDir, AppStorage) {
        let td = TempDir::new().unwrap();
        let fs = AppStorage::new(td.path().to_path_buf());
        (td, fs)
    }

    #[test]
    fn write_then_read_round_trips() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("a/b/c"), b"hello").unwrap();
        assert_eq!(
            fs.read(&rp("a/b/c")).unwrap().as_deref(),
            Some(&b"hello"[..])
        );
    }

    #[test]
    fn read_missing_is_none_not_error() {
        let (_td, fs) = fresh();
        assert!(fs.read(&rp("nope")).unwrap().is_none());
    }

    #[test]
    fn remove_missing_is_ok() {
        let (_td, mut fs) = fresh();
        // Android-side semantic: rm of a non-existent file is a no-op,
        // not an error. Matches the desktop CLI.
        fs.remove(&rp("never-existed")).unwrap();
    }

    #[test]
    fn write_creates_parent_dirs() {
        let (td, mut fs) = fresh();
        fs.write(&rp("deeply/nested/file.gpg"), b"x").unwrap();
        assert!(td.path().join("deeply/nested/file.gpg").is_file());
    }

    #[test]
    #[cfg(unix)]
    fn write_creates_with_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (td, mut fs) = fresh();
        fs.write(&rp("x.gpg"), b"ciphertext").unwrap();
        let mode = fs::metadata(td.path().join("x.gpg"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "wrote with mode {mode:#o}");
    }

    #[test]
    fn list_returns_all_files_excluding_dirs() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("a"), b"x").unwrap();
        fs.write(&rp("b/c"), b"x").unwrap();
        fs.write(&rp("b/d"), b"x").unwrap();
        let mut paths: Vec<String> = fs
            .list(None)
            .unwrap()
            .into_iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        paths.sort();
        assert_eq!(paths, vec!["a", "b/c", "b/d"]);
    }
}
