// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass backup --to <recipient>` driver.
//!
//! Streams a GPG-wrapped [`bypass_core::bundle`] tar to stdout. The
//! plaintext tar never touches disk: the [`bypass_core::bundle::write_bundle`]
//! call hands its bytes straight to `gpg`'s stdin pipe; a background
//! thread copies `gpg`'s stdout to our stdout. One entry's plaintext
//! lives in a [`bypass_core::crypto::SecretBytes`] at a time and is
//! dropped before the next entry decrypts.
//!
//! See [ADR-0026](../../../doc/adr/0026-export-import-for-backup-and-rotation.md).

use std::io::{self, Read, Write};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::crypto_gpg::{self, GpgCli};
use anyhow::{Context, Result, anyhow};
use bypass_core::bundle::{self, BundleEntry, FORMAT_VERSION, Manifest};
use bypass_core::crypto::KeyId;
use bypass_core::gpg_id;
use bypass_core::path::RelPath;

pub fn run(recipient: &str, subtree: Option<&str>) -> Result<u8> {
    let store = crate::open_store()?;
    let subtree_path = subtree
        .map(RelPath::new)
        .transpose()
        .map_err(|e| anyhow!("invalid --subtree path: {e}"))?;

    // Snapshot the entries to back up. We need the count up-front for
    // the manifest, and we want to refuse cleanly if the store is
    // empty rather than emit an empty bundle.
    let entries: Vec<RelPath> = store
        .list(subtree_path.as_ref())
        .map_err(crate::map_store_err)?;
    if entries.is_empty() {
        anyhow::bail!(
            "no entries to back up (subtree filter: {})",
            subtree.unwrap_or("<store root>")
        );
    }

    // Resolve the *current* `.gpg-id` recipients (root-level) for the
    // manifest's `original_recipients` provenance field. A dummy
    // top-level entry's parent walk hits the root `.gpg-id`.
    let dummy = RelPath::new("_").expect("`_` is a valid RelPath");
    let original_recipients: Vec<String> = gpg_id::resolve_recipients(store.storage(), &dummy)
        .map_err(|e| anyhow!("read `.gpg-id`: {e}"))?
        .into_iter()
        .map(|k: KeyId| k.as_str().to_owned())
        .collect();

    let manifest = Manifest {
        format_version: FORMAT_VERSION,
        created_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        original_recipients,
        entries: entries.len() as u32,
    };

    let gpg = GpgCli::new();
    let mut child = gpg
        .spawn_encrypt_stream(recipient)
        .with_context(|| format!("spawn gpg --encrypt --recipient {recipient}"))?;

    // Background thread: drain gpg's stdout to our stdout so gpg's
    // pipe buffer can't backpressure into stalling the tar writer.
    let mut child_stdout = child
        .stdout
        .take()
        .expect("stdout requested via Stdio::piped");
    let drain_thread = thread::spawn(move || -> io::Result<()> {
        let mut out = io::stdout().lock();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = child_stdout.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
        }
        out.flush()
    });

    let child_stdin = child
        .stdin
        .take()
        .expect("stdin requested via Stdio::piped");

    // Build a per-entry iterator that decrypts on demand. `store` is
    // borrowed for the iterator's lifetime; we drop the iterator
    // (via write_bundle returning) before reusing `store`.
    let bundle_result = {
        let store_ref = &store;
        let iter = entries.into_iter().map(|entry| {
            let plaintext = store_ref
                .show(&entry)
                .map_err(|e| bundle::BundleError::source(format!("decrypt {entry}: {e}")))?;
            Ok(BundleEntry {
                path: entry,
                plaintext,
            })
        });
        bundle::write_bundle(child_stdin, &manifest, iter)
    };

    let drain_result = drain_thread
        .join()
        .map_err(|_| anyhow!("backup output drain thread panicked"))?;
    let finish_result = crypto_gpg::finish_streaming(child);

    // Surface the first failure but make sure we wait on the child
    // either way so we don't leak a zombie. Bundle-write errors win
    // because they're the most informative.
    bundle_result.context("write backup bundle")?;
    drain_result.context("relay gpg stdout to user stdout")?;
    finish_result.context("gpg --encrypt exited with an error")?;

    let count = manifest.entries;
    let suffix = if count == 1 { "entry" } else { "entries" };
    eprintln!("backed up {count} {suffix} encrypted to {recipient}");
    Ok(0)
}
