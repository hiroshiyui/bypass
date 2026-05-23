// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass restore` driver — both fresh-store mode (default) and
//! `--in-place` mode (key rotation).
//!
//! Wire shape (ADR-0026):
//!
//! 1. Spawn `gpg --decrypt`; pipe the bundle file into its stdin
//!    from a background thread.
//! 2. Read the tar stream from `gpg`'s stdout via
//!    [`bypass_core::bundle::read_bundle`].
//! 3. For each `(RelPath, SecretBytes)` the bundle yields, call
//!    [`Store::insert_no_commit`] — which shreds any existing blob
//!    at that path first (ADR-0008) and writes the freshly-encrypted
//!    new ciphertext to the destination's current `.gpg-id`.
//! 4. After the bundle is exhausted, emit a single bulk commit so
//!    the entire restore lands as one git operation.

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use bypass_core::bundle;
use bypass_core::path::RelPath;
use bypass_core::store::Store;

use crate::crypto_gpg::{self, GpgCli};
use crate::storage_fs::StorageFs;
use crate::vcs_git2::Git2Vcs;

pub fn run(bundle_path: &Path, in_place: bool) -> Result<u8> {
    let root = StorageFs::resolve_default_root().context("resolve store root")?;

    // Both modes require a `.gpg-id` (i.e. the destination has been
    // `bypass init`'d). Fresh-store mode additionally requires the
    // store to have no entries.
    let gpg_id_path = root.join(".gpg-id");
    if !gpg_id_path.exists() {
        bail!(
            "store at {} has no `.gpg-id`; run `bypass init <recipient>` first",
            root.display(),
        );
    }

    let bundle_file = File::open(bundle_path)
        .with_context(|| format!("open bundle {}", bundle_path.display()))?;

    let mut store = crate::open_store()?;

    if !in_place {
        // Fresh-store guard: refuse if any *.gpg entries already exist.
        let existing = store.list(None).map_err(crate::map_store_err)?;
        if !existing.is_empty() {
            bail!(
                "store at {} already contains {} entr{}; refusing to restore \
                 over them. Use `bypass restore --in-place {}` to re-encrypt \
                 the existing store under the new recipient instead.",
                root.display(),
                existing.len(),
                if existing.len() == 1 { "y" } else { "ies" },
                bundle_path.display(),
            );
        }
    } else if store_has_dirty_git_state(&root) {
        bail!(
            "store at {} has an unfinished merge/rebase/cherry-pick. \
             Resolve it (`bypass git status`) before re-keying.",
            root.display(),
        );
    }

    // Drive `gpg --decrypt`: pipe bundle bytes into its stdin from a
    // background thread; read recovered tar bytes from its stdout
    // here. The bundle reader consumes `gpg`'s stdout end-to-end.
    let gpg = GpgCli::new();
    let mut child = gpg.spawn_decrypt_stream().context("spawn gpg --decrypt")?;

    let mut child_stdin = child
        .stdin
        .take()
        .expect("stdin requested via Stdio::piped");
    let pump_thread = thread::spawn(move || -> io::Result<()> {
        let mut f = bundle_file;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            child_stdin.write_all(&buf[..n])?;
        }
        // Drop child_stdin → EOF to gpg.
        Ok(())
    });

    let child_stdout = child
        .stdout
        .take()
        .expect("stdout requested via Stdio::piped");

    let mut touched_blobs: Vec<RelPath> = Vec::new();
    let read_result = bundle::read_bundle(child_stdout, bundle::no_pre_check, |entry| {
        let blob = store
            .insert_no_commit(&entry.path, entry.plaintext.as_slice())
            .map_err(|e| bundle::BundleError::source(e.to_string()))?;
        touched_blobs.push(blob);
        Ok(())
    });

    let pump_result = pump_thread
        .join()
        .map_err(|_| anyhow!("restore stdin-pump thread panicked"))?;
    let finish_result = crypto_gpg::finish_streaming(child);

    let manifest = read_result.context("read backup bundle")?;
    pump_result.context("pipe bundle bytes into gpg")?;
    finish_result.context("gpg --decrypt exited with an error")?;

    // One commit covering the whole rewrite. Fresh-store mode also
    // gets a single commit (less log noise than per-entry commits
    // for a possibly-large bundle).
    if !touched_blobs.is_empty() {
        let dest_recipient = current_recipient(&store)?;
        let message = if in_place {
            format!("bypass: Re-encrypt store for {dest_recipient}")
        } else {
            format!(
                "bypass: Restore {n} entries from backup",
                n = touched_blobs.len()
            )
        };
        store
            .commit_changes(&touched_blobs, &message)
            .map_err(crate::map_store_err)?;
    }

    let n = manifest.entries;
    let suffix = if n == 1 { "entry" } else { "entries" };
    eprintln!(
        "restored {n} {suffix} to {root}{mode}",
        root = root.display(),
        mode = if in_place { " (in-place)" } else { "" },
    );
    Ok(0)
}

/// Pull the first recipient from the destination's `.gpg-id`. Used
/// to label the rewrite commit. Falls back to a literal "new key"
/// if the file is malformed — the rewrite itself will already have
/// failed by then anyway.
fn current_recipient(store: &Store<GpgCli, StorageFs, Git2Vcs>) -> Result<String> {
    let dummy = RelPath::new("_").expect("`_` is a valid RelPath");
    let recipients = bypass_core::gpg_id::resolve_recipients(store.storage(), &dummy)
        .map_err(|e| anyhow!("read .gpg-id: {e}"))?;
    Ok(recipients
        .first()
        .map(|k| k.as_str().to_owned())
        .unwrap_or_else(|| "new key".to_owned()))
}

/// Probe for an unfinished merge/rebase/cherry-pick before doing
/// any work, so we don't half-rewrite a store and then bail when
/// `Store::commit_changes` discovers the dirty state at the end.
fn store_has_dirty_git_state(root: &Path) -> bool {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return false;
    }
    let markers = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "rebase-merge",
        "rebase-apply",
    ];
    markers.iter().any(|m| git_dir.join(m).exists())
}
