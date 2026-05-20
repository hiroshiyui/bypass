// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem-backed [`Storage`] implementation for the Linux CLI.
//!
//! Path translation is straightforward: a [`RelPath`] of `email/work.gpg`
//! becomes `<root>/email/work.gpg`. `RelPath` already forbids `..`, NULs,
//! and absolute paths, so traversal is impossible by construction; we still
//! defence-in-depth assert that the joined path starts with `root`.
//!
//! Writes are atomic (write to a sibling tempfile, fsync, then rename).
//!
//! Deletes are shred-style: the file is overwritten with three passes of
//! cryptographic random bytes (fsync between passes) before being
//! unlinked. See
//! [ADR-0008](../../../doc/adr/0008-secure-delete-via-overwrite.md) for
//! the rationale and the well-known limitations of this approach.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use bypass_core::path::RelPath;
use bypass_core::storage::Storage;
use rand::RngCore;

/// Errors emitted by [`StorageFs`].
#[derive(Debug, thiserror::Error)]
pub enum StorageFsError {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The translated absolute path escaped the configured store root.
    /// Should be impossible given `RelPath`'s invariants — this is
    /// defence-in-depth and indicates either corruption or a bug.
    #[error("path resolved outside store root: {0}")]
    OutsideRoot(PathBuf),

    /// `resolve_default_root` could not find the user's home directory and
    /// `PASSWORD_STORE_DIR` was unset.
    #[error("cannot resolve default store root: $PASSWORD_STORE_DIR unset and no home directory")]
    NoStoreRoot,
}

fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> StorageFsError {
    StorageFsError::Io {
        path: path.into(),
        source,
    }
}

/// Filesystem-backed `Storage` rooted at a known directory.
#[derive(Debug, Clone)]
pub struct StorageFs {
    root: PathBuf,
}

impl StorageFs {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Resolve the conventional store root: `$PASSWORD_STORE_DIR` if set,
    /// otherwise `~/.password-store`.
    pub fn resolve_default_root() -> Result<PathBuf, StorageFsError> {
        if let Ok(p) = std::env::var("PASSWORD_STORE_DIR")
            && !p.is_empty()
        {
            return Ok(PathBuf::from(p));
        }
        let home = dirs::home_dir().ok_or(StorageFsError::NoStoreRoot)?;
        Ok(home.join(".password-store"))
    }

    fn absolute(&self, rel: &RelPath) -> Result<PathBuf, StorageFsError> {
        let abs = self.root.join(rel.as_str());
        // Defence-in-depth: RelPath should already guarantee this, but
        // if it ever changes, refuse rather than touch arbitrary paths.
        if !abs.starts_with(&self.root) {
            return Err(StorageFsError::OutsideRoot(abs));
        }
        Ok(abs)
    }
}

impl Storage for StorageFs {
    type Error = StorageFsError;

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
        // Atomic write: tempfile in the destination's directory, then rename.
        let parent = abs.parent().unwrap_or_else(|| Path::new("."));
        let tmp_path = unique_tmp_path(parent, abs.file_name().unwrap_or_default());
        let mut tmp_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|e| io_err(tmp_path.clone(), e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&tmp_path, fs::Permissions::from_mode(0o600));
        }
        let write_res = tmp_file.write_all(data).and_then(|()| tmp_file.sync_all());
        drop(tmp_file);
        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp_path);
            return Err(io_err(tmp_path, e));
        }
        if let Err(e) = fs::rename(&tmp_path, &abs) {
            let _ = fs::remove_file(&tmp_path);
            return Err(io_err(abs, e));
        }
        Ok(())
    }

    fn remove(&mut self, path: &RelPath) -> Result<(), Self::Error> {
        let abs = self.absolute(path)?;
        if !abs.exists() {
            return Ok(());
        }
        overwrite_then_unlink(&abs)
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

fn walk(dir: &Path, root: &Path, out: &mut Vec<RelPath>) -> Result<(), StorageFsError> {
    let read = fs::read_dir(dir).map_err(|e| io_err(dir.to_owned(), e))?;
    for entry in read {
        let entry = entry.map_err(|e| io_err(dir.to_owned(), e))?;
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        // Skip git's own directory so listing a pass-style store doesn't
        // surface thousands of internal objects. The `.git/` tree is the
        // backend's concern, not part of the blob namespace.
        if name == ".git" {
            continue;
        }
        let file_type = entry.file_type().map_err(|e| io_err(path.clone(), e))?;
        if file_type.is_dir() {
            walk(&path, root, out)?;
        } else if file_type.is_file()
            && let Ok(rel) = path.strip_prefix(root)
        {
            // Reject relative paths whose UTF-8 form can't be a RelPath
            // (e.g. names containing a NUL or a `..` segment that
            // somehow slipped past validation). Silently skipping is
            // acceptable: the trait's contract is "blob paths" and an
            // unrepresentable name doesn't fit.
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if let Ok(rp) = RelPath::new(rel_str) {
                out.push(rp);
            }
        }
    }
    Ok(())
}

// ----- shred-style remove (ADR-0008) -----------------------------------

/// Number of cryptographic-random overwrite passes before unlink.
/// Mirrors GNU `shred(1)`'s default.
pub(crate) const SHRED_PASSES: usize = 3;

/// Overwrite `path` with random bytes [`SHRED_PASSES`] times and unlink it.
///
/// Exposed at crate visibility so `bypass edit` (Milestone 1.3, Commit 3)
/// can wipe its tempfile through the same path.
pub(crate) fn overwrite_then_unlink(path: &Path) -> Result<(), StorageFsError> {
    overwrite_in_place(path, SHRED_PASSES)?;
    fs::remove_file(path).map_err(|e| io_err(path.to_owned(), e))
}

fn overwrite_in_place(path: &Path, passes: usize) -> Result<(), StorageFsError> {
    let len = fs::metadata(path)
        .map_err(|e| io_err(path.to_owned(), e))?
        .len();
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| io_err(path.to_owned(), e))?;
    if len == 0 {
        // Nothing to overwrite; sync to make sure no buffered metadata
        // change is racing the unlink.
        return file.sync_all().map_err(|e| io_err(path.to_owned(), e));
    }
    let mut buf = [0u8; 4096];
    let mut rng = rand::rng();
    for _ in 0..passes {
        file.seek(SeekFrom::Start(0))
            .map_err(|e| io_err(path.to_owned(), e))?;
        let mut remaining = len;
        while remaining > 0 {
            let chunk = remaining.min(buf.len() as u64) as usize;
            rng.fill_bytes(&mut buf[..chunk]);
            file.write_all(&buf[..chunk])
                .map_err(|e| io_err(path.to_owned(), e))?;
            remaining -= chunk as u64;
        }
        file.sync_all().map_err(|e| io_err(path.to_owned(), e))?;
    }
    Ok(())
}

// ----- atomic write tempfile helper -------------------------------------

fn unique_tmp_path(dir: &Path, dest_name: &std::ffi::OsStr) -> PathBuf {
    use std::time::SystemTime;
    let mut name = dest_name.to_os_string();
    name.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0),
    ));
    dir.join(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn fresh() -> (TempDir, StorageFs) {
        let td = TempDir::new().unwrap();
        let fs = StorageFs::new(td.path().to_path_buf());
        (td, fs)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("a/b/c"), b"hello").unwrap();
        assert_eq!(
            fs.read(&rp("a/b/c")).unwrap().as_deref(),
            Some(&b"hello"[..])
        );
    }

    #[test]
    fn read_missing_returns_none() {
        let (_td, fs) = fresh();
        assert!(fs.read(&rp("nope")).unwrap().is_none());
    }

    #[test]
    fn exists_reports_correctly() {
        let (_td, mut fs) = fresh();
        assert!(!fs.exists(&rp("e")).unwrap());
        fs.write(&rp("e"), b"x").unwrap();
        assert!(fs.exists(&rp("e")).unwrap());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let (td, mut fs) = fresh();
        fs.write(&rp("deeply/nested/dir/file"), b"x").unwrap();
        assert!(td.path().join("deeply/nested/dir/file").is_file());
    }

    #[test]
    fn remove_unlinks_the_file() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("e"), b"x").unwrap();
        fs.remove(&rp("e")).unwrap();
        assert!(!fs.exists(&rp("e")).unwrap());
    }

    #[test]
    fn remove_missing_is_ok() {
        let (_td, mut fs) = fresh();
        fs.remove(&rp("never-existed")).unwrap();
    }

    #[test]
    fn overwrite_in_place_replaces_contents() {
        let (td, mut fs) = fresh();
        let original = b"this is a sensitive secret string";
        fs.write(&rp("secret"), original).unwrap();
        let abs = td.path().join("secret");
        overwrite_in_place(&abs, 3).unwrap();
        let after = fs::read(&abs).unwrap();
        assert_eq!(after.len(), original.len(), "length preserved");
        assert_ne!(after, original, "contents must be replaced after overwrite");
    }

    #[test]
    fn list_finds_files_recursively() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("a"), b"1").unwrap();
        fs.write(&rp("b/c"), b"2").unwrap();
        fs.write(&rp("b/d/e"), b"3").unwrap();
        let mut got: Vec<String> = fs
            .list(None)
            .unwrap()
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        got.sort();
        assert_eq!(got, vec!["a", "b/c", "b/d/e"]);
    }

    #[test]
    fn list_with_prefix_scopes_to_subtree() {
        let (_td, mut fs) = fresh();
        fs.write(&rp("a"), b"1").unwrap();
        fs.write(&rp("b/c"), b"2").unwrap();
        fs.write(&rp("b/d"), b"3").unwrap();
        let mut got: Vec<String> = fs
            .list(Some(&rp("b")))
            .unwrap()
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        got.sort();
        assert_eq!(got, vec!["b/c", "b/d"]);
    }

    #[test]
    fn list_skips_git_directory() {
        let (td, mut fs) = fresh();
        fs.write(&rp("a"), b"1").unwrap();
        let git_dir = td.path().join(".git").join("objects");
        fs::create_dir_all(&git_dir).unwrap();
        fs::write(git_dir.join("noise"), b"x").unwrap();
        let got: Vec<String> = fs
            .list(None)
            .unwrap()
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        assert_eq!(got, vec!["a".to_owned()]);
    }

    #[test]
    fn list_of_missing_prefix_is_empty() {
        let (_td, fs) = fresh();
        assert!(fs.list(Some(&rp("nothing"))).unwrap().is_empty());
    }

    #[test]
    fn resolve_default_root_prefers_env_var() {
        // We can't use serial tests without a dep, so just verify the
        // explicit-env-var path. Calling resolve_default_root() with the
        // env var set is sufficient because the function checks the var
        // first.
        // SAFETY: single-threaded test; we restore the variable below.
        let prev = std::env::var("PASSWORD_STORE_DIR").ok();
        // SAFETY: see above.
        unsafe { std::env::set_var("PASSWORD_STORE_DIR", "/tmp/bypass-test-explicit") };
        let r = StorageFs::resolve_default_root().unwrap();
        assert_eq!(r, PathBuf::from("/tmp/bypass-test-explicit"));
        match prev {
            // SAFETY: see above.
            Some(p) => unsafe { std::env::set_var("PASSWORD_STORE_DIR", p) },
            None => unsafe { std::env::remove_var("PASSWORD_STORE_DIR") },
        }
    }
}
