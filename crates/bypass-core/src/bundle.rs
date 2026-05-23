// SPDX-License-Identifier: GPL-3.0-or-later

//! Backup-bundle format used by `bypass backup` and `bypass restore`
//! ([ADR-0026](../../../doc/adr/0026-export-import-for-backup-and-rotation.md)).
//!
//! A bundle is a ustar archive with this shape:
//!
//! ```text
//! manifest.toml          # first entry; serialised [`Manifest`]
//! entries/<RelPath>      # one tar member per password entry, raw plaintext
//! ```
//!
//! `bypass backup` streams the tar through `gpg --encrypt --recipient
//! <to>` so the final on-disk artefact is a single OpenPGP-wrapped tar.
//! `bypass restore` reverses that: `gpg --decrypt` → this reader.
//!
//! This module is **pure logic** — it operates on `Read` / `Write`
//! handles supplied by the frontend. No filesystem, no subprocesses
//! ([ADR-0003](../../../doc/adr/0003-workspace-split-core-cli.md)).
//! Plaintext flows through [`crate::crypto::SecretBytes`] so the
//! frontend can keep `SecretBytes`'s zeroize-on-drop guarantee end-to-
//! end; the inside of this module copies bytes through stack scratch
//! buffers that are dropped as each entry is processed.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::crypto::SecretBytes;
use crate::path::RelPath;

/// Manifest schema version. Bump on incompatible changes — readers
/// refuse anything that isn't this exact value.
pub const FORMAT_VERSION: u32 = 1;

/// Tar member name for the manifest. Must be the first archive
/// member; readers refuse bundles where it isn't.
const MANIFEST_NAME: &str = "manifest.toml";

/// Prefix under which entry plaintexts live in the tar.
const ENTRIES_PREFIX: &str = "entries/";

/// Bundle preamble. Sealed inside the outer GPG wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Wire-format version. See [`FORMAT_VERSION`].
    pub format_version: u32,
    /// Unix epoch seconds when the bundle was produced. Recorded for
    /// provenance only — not used for ordering.
    pub created_at_unix: i64,
    /// Recipients listed in the `.gpg-id` walked from the source
    /// store at backup time. Captured for the human reading the
    /// manifest later; never used to gate restore behaviour.
    pub original_recipients: Vec<String>,
    /// Number of password entries in the bundle.
    pub entries: u32,
}

#[derive(Debug, Error)]
pub enum BundleError {
    #[error("bundle I/O: {0}")]
    Io(#[from] std::io::Error),

    #[error("manifest serialisation: {0}")]
    EncodeManifest(#[from] toml::ser::Error),

    #[error("manifest parse: {0}")]
    DecodeManifest(#[from] toml::de::Error),

    #[error("manifest is not valid UTF-8")]
    NonUtf8Manifest,

    #[error("bundle format_version {got} is not supported (this build expects {expected})")]
    IncompatibleFormat { got: u32, expected: u32 },

    #[error("bundle is missing the manifest entry (expected `{MANIFEST_NAME}` first)")]
    MissingManifest,

    #[error("bundle entry path is invalid: {0}")]
    InvalidEntryPath(String),

    #[error("bundle tar member name is not UTF-8")]
    NonUtf8MemberName,

    #[error(
        "unexpected tar member `{0}` (only `{MANIFEST_NAME}` and `{ENTRIES_PREFIX}*` are allowed)"
    )]
    UnexpectedMember(String),

    /// An iterator passed to [`write_bundle`] (or a visitor passed
    /// to [`read_bundle`]) failed. Carries the caller's error
    /// preserved as a trait object so the [`write_bundle`] surface
    /// can stay generic over what *kind* of upstream error a CLI
    /// driver produces — typically a store-level `anyhow::Error`
    /// from decrypting an individual entry.
    #[error("bundle source: {0}")]
    Source(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl BundleError {
    /// Wrap any error as [`BundleError::Source`] without naming it.
    /// Use this in CLI code to convert per-entry errors into a
    /// shape `write_bundle`'s iterator can yield.
    pub fn source<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> Self {
        Self::Source(e.into())
    }
}

/// One password entry as it travels through the bundle: logical
/// `RelPath` (with no `.gpg` suffix — the bundle holds plaintexts)
/// and the plaintext bytes wrapped for zeroize-on-drop.
pub struct BundleEntry {
    pub path: RelPath,
    pub plaintext: SecretBytes,
}

// ===== writer =========================================================

/// Stream a bundle to `sink`. Writes `manifest.toml` first, then one
/// `entries/<path>` tar member per entry from the iterator. Iterator
/// items may themselves fail (a per-entry decrypt error, say) — the
/// `Result` short-circuits the write.
///
/// `sink` will typically be `gpg`'s stdin in the CLI; for tests it can
/// be any `Write`. The caller drives the outer encryption.
pub fn write_bundle<W: Write, I>(
    sink: W,
    manifest: &Manifest,
    entries: I,
) -> Result<(), BundleError>
where
    I: IntoIterator<Item = Result<BundleEntry, BundleError>>,
{
    let mut builder = tar::Builder::new(sink);
    builder.mode(tar::HeaderMode::Deterministic);

    let manifest_bytes = toml::to_string(manifest)?.into_bytes();
    append_member(&mut builder, MANIFEST_NAME, &manifest_bytes)?;

    for item in entries {
        let entry = item?;
        let member_name = format!("{ENTRIES_PREFIX}{}", entry.path.as_str());
        append_member(&mut builder, &member_name, entry.plaintext.as_slice())?;
    }

    builder.finish()?;
    let mut sink = builder.into_inner()?;
    sink.flush()?;
    Ok(())
}

fn append_member<W: Write>(
    builder: &mut tar::Builder<W>,
    name: &str,
    data: &[u8],
) -> Result<(), BundleError> {
    let mut header = tar::Header::new_ustar();
    header.set_size(data.len() as u64);
    header.set_mode(0o600);
    header.set_entry_type(tar::EntryType::Regular);
    header.set_cksum();
    builder.append_data(&mut header, name, data)?;
    Ok(())
}

// ===== reader =========================================================

/// Read a bundle from `source` in one pass. Consumes the first tar
/// member as `manifest.toml`, verifies `format_version`, then calls
/// `visit` for each entry tar member. Returns the parsed manifest.
///
/// The tar crate's `Entries` iterator borrows the underlying
/// `Archive`, so we can't hand the manifest back before iteration
/// without giving up streaming or going through self-referential
/// gymnastics. The `pre_check` closure is the way to refuse a bundle
/// before any per-entry visit fires (e.g. surface a custom error in
/// the CLI's "this version of bypass can't read this bundle" path);
/// pass [`no_pre_check`] for the common case.
pub fn read_bundle<R, P, F>(source: R, pre_check: P, mut visit: F) -> Result<Manifest, BundleError>
where
    R: Read,
    P: FnOnce(&Manifest) -> Result<(), BundleError>,
    F: FnMut(BundleEntry) -> Result<(), BundleError>,
{
    let mut archive = tar::Archive::new(source);
    let mut entries_iter = archive.entries()?;

    // --- first member: manifest -------------------------------------
    let Some(first) = entries_iter.next() else {
        return Err(BundleError::MissingManifest);
    };
    let mut first = first?;
    let name = member_name(&first)?;
    if name != MANIFEST_NAME {
        return Err(BundleError::MissingManifest);
    }
    let mut buf = Vec::with_capacity(first.size() as usize);
    first.read_to_end(&mut buf)?;
    let manifest_text = std::str::from_utf8(&buf).map_err(|_| BundleError::NonUtf8Manifest)?;
    let manifest: Manifest = toml::from_str(manifest_text)?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(BundleError::IncompatibleFormat {
            got: manifest.format_version,
            expected: FORMAT_VERSION,
        });
    }
    pre_check(&manifest)?;

    // --- subsequent members: entries --------------------------------
    for member in entries_iter {
        let mut member = member?;
        let name = member_name(&member)?;
        let rel = name
            .strip_prefix(ENTRIES_PREFIX)
            .ok_or_else(|| BundleError::UnexpectedMember(name.clone()))?;
        let path = RelPath::new(rel.to_owned())
            .map_err(|e| BundleError::InvalidEntryPath(e.to_string()))?;
        let mut buf = Vec::with_capacity(member.size() as usize);
        member.read_to_end(&mut buf)?;
        visit(BundleEntry {
            path,
            plaintext: SecretBytes::new(buf),
        })?;
    }

    Ok(manifest)
}

/// Trivial pre-check that accepts any manifest. Pass this to
/// [`read_bundle`] when only format-version compatibility matters.
pub fn no_pre_check(_: &Manifest) -> Result<(), BundleError> {
    Ok(())
}

fn member_name<R: Read>(entry: &tar::Entry<'_, R>) -> Result<String, BundleError> {
    let path = entry.path()?;
    path.to_str()
        .map(|s| s.to_owned())
        .ok_or(BundleError::NonUtf8MemberName)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn sample_manifest(n: u32) -> Manifest {
        Manifest {
            format_version: FORMAT_VERSION,
            created_at_unix: 1_779_410_123,
            original_recipients: vec!["ABCD1234".into(), "EFEF5678".into()],
            entries: n,
        }
    }

    fn entry(path: &str, body: &[u8]) -> Result<BundleEntry, BundleError> {
        Ok(BundleEntry {
            path: rp(path),
            plaintext: SecretBytes::new(body.to_vec()),
        })
    }

    #[test]
    fn round_trip_preserves_manifest_and_entries() {
        let mut buf = Vec::new();
        let manifest = sample_manifest(3);
        write_bundle(
            &mut buf,
            &manifest,
            vec![
                entry("email/work", b"hunter2"),
                entry("banking/chase", b"multi\nline\npass"),
                entry("notes", b""),
            ],
        )
        .unwrap();

        let mut got = Vec::new();
        let returned = read_bundle(&buf[..], no_pre_check, |e| {
            got.push((e.path.as_str().to_owned(), e.plaintext.as_slice().to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(returned, manifest);

        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![
                ("banking/chase".to_owned(), b"multi\nline\npass".to_vec()),
                ("email/work".to_owned(), b"hunter2".to_vec()),
                ("notes".to_owned(), b"".to_vec()),
            ]
        );
    }

    #[test]
    fn round_trip_empty_store() {
        let mut buf = Vec::new();
        let manifest = sample_manifest(0);
        write_bundle(&mut buf, &manifest, std::iter::empty()).unwrap();

        let returned =
            read_bundle(&buf[..], no_pre_check, |_| panic!("no entries expected")).unwrap();
        assert_eq!(returned.entries, 0);
    }

    fn read_err(buf: &[u8]) -> BundleError {
        match read_bundle(buf, no_pre_check, |_| Ok(())) {
            Ok(_) => panic!("expected read_bundle() to fail"),
            Err(e) => e,
        }
    }

    #[test]
    fn read_rejects_incompatible_format_version() {
        let mut buf = Vec::new();
        let bogus = Manifest {
            format_version: 999,
            ..sample_manifest(0)
        };
        write_bundle(&mut buf, &bogus, std::iter::empty()).unwrap();

        let err = read_err(&buf);
        assert!(
            matches!(
                err,
                BundleError::IncompatibleFormat {
                    got: 999,
                    expected: FORMAT_VERSION
                }
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn read_rejects_archive_without_manifest_first() {
        let mut buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut buf);
            let payload = b"oops";
            let mut h = tar::Header::new_ustar();
            h.set_size(payload.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, "entries/x", &payload[..]).unwrap();
            b.finish().unwrap();
        }
        let err = read_err(&buf);
        assert!(matches!(err, BundleError::MissingManifest), "got {err:?}");
    }

    #[test]
    fn read_rejects_unexpected_member_path() {
        let mut buf = Vec::new();
        let m = sample_manifest(0);
        let manifest_bytes = toml::to_string(&m).unwrap().into_bytes();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut h = tar::Header::new_ustar();
            h.set_size(manifest_bytes.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, "manifest.toml", &manifest_bytes[..])
                .unwrap();
            let stray = b"oops";
            let mut h = tar::Header::new_ustar();
            h.set_size(stray.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, "stray.txt", &stray[..]).unwrap();
            b.finish().unwrap();
        }
        let err = read_err(&buf);
        assert!(
            matches!(err, BundleError::UnexpectedMember(ref n) if n == "stray.txt"),
            "got {err:?}"
        );
    }

    #[test]
    fn read_rejects_relpath_invalid_member_name() {
        // The tar crate itself refuses `..` in member names, so we
        // can't smuggle a traversal that way — but a member name
        // with a backslash is accepted by tar and rejected by
        // RelPath. Confirms the reader honours RelPath's full
        // validation surface (ADR-0007), not just `..` handling.
        let mut buf = Vec::new();
        let m = sample_manifest(0);
        let manifest_bytes = toml::to_string(&m).unwrap().into_bytes();
        {
            let mut b = tar::Builder::new(&mut buf);
            let mut h = tar::Header::new_ustar();
            h.set_size(manifest_bytes.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, "manifest.toml", &manifest_bytes[..])
                .unwrap();
            let payload = b"x";
            let mut h = tar::Header::new_ustar();
            h.set_size(payload.len() as u64);
            h.set_mode(0o600);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append_data(&mut h, r"entries/email\work", &payload[..])
                .unwrap();
            b.finish().unwrap();
        }
        let err = read_err(&buf);
        assert!(
            matches!(err, BundleError::InvalidEntryPath(_)),
            "got {err:?}"
        );
    }
}
