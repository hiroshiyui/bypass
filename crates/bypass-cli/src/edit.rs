// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass edit`: decrypt an entry into a tempfile, open `$EDITOR`,
//! re-encrypt the result.
//!
//! Workspace layout (security audit findings E1 + E2 informed this):
//!
//! - Each `bypass edit` invocation creates its own fresh 0700-mode
//!   subdirectory under `/dev/shm` (tmpfs, preferred) or
//!   `std::env::temp_dir()`. The tempfile holding the plaintext lives
//!   inside that dir, created with `mode = 0o600` atomically via
//!   `OpenOptionsExt::mode` (no chmod-after-create TOCTOU window —
//!   E1).
//! - On exit (normal, error, panic), the [`Drop`] impl on
//!   [`EditWorkspace`] shred-overwrites **every** file in the dir and
//!   then removes the dir itself. Catches editor swap / backup files
//!   (`.work.gpg.swp`, `#work.gpg#`, `work.gpg~`, …) that the editor
//!   may have left next to ours — E2.
//!
//! The shred routine is `StorageFs::overwrite_then_unlink` per
//! [ADR-0008](../../../doc/adr/0008-secure-delete-via-overwrite.md).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

/// A scratch directory for one `bypass edit` invocation. The directory
/// is mode 0700 so no sibling user can read its contents; the
/// plaintext tempfile sits inside at mode 0600. On drop, every file
/// in the directory (the tempfile *plus* any editor swap/backup
/// file the editor left next to it) is shred-overwritten, then the
/// directory is removed. Best-effort: failures in Drop are swallowed
/// because Drop has nowhere to surface them.
struct EditWorkspace {
    dir: PathBuf,
    file: PathBuf,
}

impl Drop for EditWorkspace {
    fn drop(&mut self) {
        // Shred every file the editor might have created in our dir.
        if let Ok(read) = fs::read_dir(&self.dir) {
            for entry in read.flatten() {
                let p = entry.path();
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    let _ = overwrite_then_unlink(&p);
                }
            }
        }
        let _ = fs::remove_dir(&self.dir);
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

    // 2. Stage the plaintext into a fresh 0700 scratch dir + 0600 file.
    let workspace = stage_workspace(entry, &existing)?;

    // 3. Spawn $EDITOR (or vi).
    spawn_editor(&workspace.file)?;

    // 4. Re-read the (possibly edited) tempfile.
    let new_plaintext: Zeroizing<Vec<u8>> = Zeroizing::new(
        fs::read(&workspace.file)
            .with_context(|| format!("read tempfile {}", workspace.file.display()))?,
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

    // workspace (dir + every file in it) is shredded by Drop.
    drop(workspace);
    Ok(())
}

fn stage_workspace(entry: &RelPath, plaintext: &[u8]) -> Result<EditWorkspace> {
    // Per-edit subdirectory under the chosen tempdir so editor swap /
    // backup files land *next to ours* in a 0700 enclosure that our
    // Drop impl owns. Avoids leaking partial plaintext into a shared
    // tmpfs after a panic-mid-edit. (Audit finding E2.)
    let parent = pick_tempdir()?;
    let dir = parent.join(unique_dir_name());
    fs::create_dir(&dir).with_context(|| format!("create workspace dir {}", dir.display()))?;
    let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));

    let file = dir.join(file_name(entry));
    // Atomic create-with-mode: `OpenOptionsExt::mode(0o600)` passes the
    // mode through the `open(2)` syscall, so the file never exists at
    // umask-default before chmod (audit finding E1). open(2) masks by
    // umask; all bits in 0o600 are owner-only, so any sane umask
    // preserves them.
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&file)
        .with_context(|| format!("create tempfile {}", file.display()))?;
    f.write_all(plaintext)
        .with_context(|| format!("write tempfile {}", file.display()))?;
    f.sync_all()
        .with_context(|| format!("sync tempfile {}", file.display()))?;
    Ok(EditWorkspace { dir, file })
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

/// Sanitised file name for the tempfile (no slashes, no backslashes).
/// The entry's own file_name() is already path-component-safe per
/// `RelPath`'s invariants; this is just for the visible-in-$EDITOR
/// label so users see `github.com` not `email.github.com`.
fn file_name(entry: &RelPath) -> String {
    entry.file_name().replace(['/', '\\'], "_")
}

/// Per-edit workspace directory name. Uniqueness is local to one
/// process; create-with-EEXIST refuses on collision so a hostile
/// pre-create is harmless.
fn unique_dir_name() -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("bypass-edit.{}.{nanos}", std::process::id())
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
