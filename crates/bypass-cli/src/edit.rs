// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass edit`: decrypt an entry into a tempfile, open `$EDITOR`,
//! re-encrypt the result.
//!
//! Tempfile placement:
//!
//! - `/dev/shm` (tmpfs) on Linux when present and writable, so the
//!   plaintext lives in RAM-backed storage.
//! - `std::env::temp_dir()` as a fallback elsewhere.
//!
//! The tempfile is chmod'd 0600 and wiped through the same shred-style
//! path as `StorageFs::remove` (see ADR-0008) via a `Drop` guard, so a
//! panic or early return between writing the plaintext and the
//! re-encrypt step still scrubs the file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use bypass_core::crypto::Crypto;
use bypass_core::path::RelPath;
use bypass_core::storage::Storage;
use bypass_core::store::{Store, StoreError};
use bypass_core::vcs::VersionControl;
use zeroize::Zeroizing;

use crate::storage_fs::overwrite_then_unlink;

/// A path on disk that owns its lifetime: when dropped, the file at
/// that path is shred-overwritten and unlinked. Best-effort — failures
/// are swallowed because Drop has nowhere to surface them.
struct ShreddingTempfile {
    path: PathBuf,
}

impl Drop for ShreddingTempfile {
    fn drop(&mut self) {
        if self.path.exists() {
            let _ = overwrite_then_unlink(&self.path);
        }
    }
}

pub fn run<C, S, V>(store: &mut Store<C, S, V>, entry: &RelPath) -> Result<()>
where
    C: Crypto,
    S: Storage,
    V: VersionControl,
    C::Error: 'static,
    S::Error: 'static,
    V::Error: 'static,
{
    // 1. Load current plaintext (empty for new entries — pass behaviour).
    //    Wrap in `Zeroizing` so the heap copy is scrubbed on drop
    //    (security audit H1).
    let existing: Zeroizing<Vec<u8>> = match store.show(entry) {
        Ok(bytes) => Zeroizing::new(bytes.as_slice().to_vec()),
        Err(StoreError::NotFound(_)) => Zeroizing::new(Vec::new()),
        Err(e) => return Err(anyhow::Error::new(e)),
    };

    // 2. Stage the plaintext to a tempfile and chmod 0600.
    let tmp = stage_tempfile(entry, &existing)?;

    // 3. Spawn $EDITOR (or vi).
    spawn_editor(&tmp.path)?;

    // 4. Re-read the (possibly edited) tempfile.
    let new_plaintext: Zeroizing<Vec<u8>> = Zeroizing::new(
        fs::read(&tmp.path).with_context(|| format!("read tempfile {}", tmp.path.display()))?,
    );

    // 5. Compare and persist if changed.
    if *new_plaintext == *existing {
        eprintln!("no changes");
        return Ok(());
    }
    store
        .insert(entry, &new_plaintext, /*overwrite=*/ true)
        .map_err(anyhow::Error::new)
        .with_context(|| format!("save edited entry {entry}"))?;

    // tmp is shredded by Drop.
    drop(tmp);
    Ok(())
}

fn stage_tempfile(entry: &RelPath, plaintext: &[u8]) -> Result<ShreddingTempfile> {
    let dir = pick_tempdir()?;
    let name = unique_name(entry);
    let path = dir.join(name);

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("create tempfile {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // chmod before writing so the window where the file is world-
        // readable is as narrow as possible.
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    file.write_all(plaintext)
        .with_context(|| format!("write tempfile {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("sync tempfile {}", path.display()))?;
    Ok(ShreddingTempfile { path })
}

fn pick_tempdir() -> Result<PathBuf> {
    let shm = Path::new("/dev/shm");
    if shm.is_dir() {
        // Probe write permission by creating-and-removing a probe file.
        let probe = shm.join(format!("bypass.probe.{}", std::process::id()));
        if let Ok(f) = OpenOptions::new().write(true).create_new(true).open(&probe) {
            drop(f);
            let _ = fs::remove_file(&probe);
            return Ok(shm.to_path_buf());
        }
    }
    let fallback = std::env::temp_dir();
    if fallback.is_dir() {
        Ok(fallback)
    } else {
        bail!("no writable tempdir available (/dev/shm and TMPDIR both unusable)")
    }
}

fn unique_name(entry: &RelPath) -> String {
    let stem = entry.file_name().replace(['/', '\\'], "_");
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("bypass.edit.{}.{nanos}.{stem}", std::process::id())
}

fn spawn_editor(path: &Path) -> Result<()> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let escaped = path.to_string_lossy().replace('\'', r"'\''");
    let cmd = format!("{editor} '{escaped}'");
    let status = Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .status()
        .with_context(|| format!("spawn editor via `sh -c {cmd:?}`"))?;
    if !status.success() {
        bail!("editor exited with status {status}");
    }
    Ok(())
}
