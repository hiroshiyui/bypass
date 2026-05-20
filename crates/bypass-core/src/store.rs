// SPDX-License-Identifier: GPL-3.0-or-later

//! High-level password store orchestrator. Generic over the platform's
//! [`Crypto`], [`Storage`], and [`VersionControl`] implementations
//! (see [ADR-0006](../../../doc/adr/0006-trait-associated-error-types.md)).
//!
//! Entry naming uses the pass convention: a logical entry `email/work` is
//! stored as a blob at `email/work.gpg`. `.gpg-id` files are placed at the
//! store root (and optionally per-subtree) to declare recipients
//! (see [ADR-0002](../../../doc/adr/0002-pass-compatible-on-disk-layout.md)).

use crate::crypto::{Crypto, KeyId, SecretBytes};
use crate::gpg_id::{self, GpgIdError};
use crate::path::RelPath;
use crate::storage::Storage;
use crate::vcs::VersionControl;

/// Failures from [`Store`] operations.
///
/// Wraps the three backend errors and adds a small number of orchestrator-
/// level variants (`NotFound`, `AlreadyExists`, `NotInitialized`,
/// `GpgIdMalformed`) so the CLI can render specific messages without
/// having to downcast.
#[derive(Debug, thiserror::Error)]
pub enum StoreError<CE, SE, VE>
where
    CE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    VE: std::error::Error + Send + Sync + 'static,
{
    #[error("entry not found: {0}")]
    NotFound(String),

    #[error("entry already exists: {0}")]
    AlreadyExists(String),

    #[error("password store has no .gpg-id; run `bypass init <gpg-id>` first")]
    NotInitialized,

    #[error("malformed .gpg-id: {0}")]
    GpgIdMalformed(&'static str),

    #[error("crypto: {0}")]
    Crypto(#[source] CE),

    #[error("storage: {0}")]
    Storage(#[source] SE),

    #[error("version control: {0}")]
    Vcs(#[source] VE),
}

impl<CE, SE, VE> StoreError<CE, SE, VE>
where
    CE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    VE: std::error::Error + Send + Sync + 'static,
{
    fn from_gpg_id(e: GpgIdError<SE>) -> Self {
        match e {
            GpgIdError::Missing => Self::NotInitialized,
            GpgIdError::Empty => Self::GpgIdMalformed("no recipients"),
            GpgIdError::NotUtf8 => Self::GpgIdMalformed("not UTF-8"),
            GpgIdError::Storage(se) => Self::Storage(se),
        }
    }
}

/// Convenience alias for `Result<T, StoreError<C::Error, S::Error, V::Error>>`.
pub type Result<T, C, S, V> = std::result::Result<
    T,
    StoreError<<C as Crypto>::Error, <S as Storage>::Error, <V as VersionControl>::Error>,
>;

/// The password-store orchestrator.
pub struct Store<C, S, V> {
    crypto: C,
    storage: S,
    vcs: V,
}

impl<C, S, V> Store<C, S, V>
where
    C: Crypto,
    S: Storage,
    V: VersionControl,
{
    pub fn new(crypto: C, storage: S, vcs: V) -> Self {
        Self {
            crypto,
            storage,
            vcs,
        }
    }

    /// Borrow the underlying [`Storage`]. Useful for backend-specific
    /// concerns (e.g. wiping a tempfile through the same secure-delete
    /// path the store uses).
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Initialise the store: write `.gpg-id` with `recipients` (one per
    /// line) and ask the [`VersionControl`] to initialise the repository.
    /// An initial commit is created for the `.gpg-id`. Both `vcs.init` and
    /// `vcs.commit` are no-ops under [`crate::vcs::NoVcs`].
    pub fn init(&mut self, recipients: &[KeyId]) -> Result<(), C, S, V> {
        if recipients.is_empty() {
            return Err(StoreError::GpgIdMalformed("no recipients"));
        }
        let gpg_id_path = gpg_id_path();
        let mut body = String::new();
        for r in recipients {
            body.push_str(r.as_str());
            body.push('\n');
        }
        self.storage
            .write(&gpg_id_path, body.as_bytes())
            .map_err(StoreError::Storage)?;
        self.vcs.init().map_err(StoreError::Vcs)?;
        self.vcs
            .commit(&[gpg_id_path], "bypass: initialise store")
            .map_err(StoreError::Vcs)?;
        Ok(())
    }

    /// Encrypt `plaintext` to the recipients resolved for `entry` and
    /// write the ciphertext to the blob at `entry.gpg`.
    pub fn insert(
        &mut self,
        entry: &RelPath,
        plaintext: &[u8],
        overwrite: bool,
    ) -> Result<(), C, S, V> {
        let blob = entry_to_blob(entry);
        if !overwrite && self.storage.exists(&blob).map_err(StoreError::Storage)? {
            return Err(StoreError::AlreadyExists(entry.as_str().to_owned()));
        }
        let recipients =
            gpg_id::resolve_recipients(&self.storage, entry).map_err(StoreError::from_gpg_id)?;
        let ciphertext = self
            .crypto
            .encrypt(plaintext, &recipients)
            .map_err(StoreError::Crypto)?;
        self.storage
            .write(&blob, &ciphertext)
            .map_err(StoreError::Storage)?;
        let verb = if overwrite { "Update" } else { "Add" };
        self.vcs
            .commit(&[blob], &format!("bypass: {verb} password for {entry}"))
            .map_err(StoreError::Vcs)?;
        Ok(())
    }

    /// Decrypt and return the plaintext of `entry`.
    pub fn show(&self, entry: &RelPath) -> Result<SecretBytes, C, S, V> {
        let blob = entry_to_blob(entry);
        let ciphertext = self
            .storage
            .read(&blob)
            .map_err(StoreError::Storage)?
            .ok_or_else(|| StoreError::NotFound(entry.as_str().to_owned()))?;
        self.crypto.decrypt(&ciphertext).map_err(StoreError::Crypto)
    }

    /// All entries below `subpath` (or below the store root when `None`),
    /// sorted lexicographically. Entry paths are returned without their
    /// on-disk `.gpg` suffix. Non-entry files (`.gpg-id`, anything not
    /// ending in `.gpg`) are filtered out.
    pub fn list(&self, subpath: Option<&RelPath>) -> Result<Vec<RelPath>, C, S, V> {
        let blobs = self.storage.list(subpath).map_err(StoreError::Storage)?;
        let mut entries: Vec<RelPath> = blobs.iter().filter_map(blob_to_entry).collect();
        entries.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        Ok(entries)
    }

    /// All entries whose path contains `pattern` as a substring.
    pub fn find(&self, pattern: &str) -> Result<Vec<RelPath>, C, S, V> {
        let entries = self.list(None)?;
        Ok(entries
            .into_iter()
            .filter(|e| e.as_str().contains(pattern))
            .collect())
    }

    /// Delete a single entry. Returns `NotFound` if the entry does not
    /// exist. Delete semantics (e.g. shred-style overwrite on `StorageFs`,
    /// see ADR-0008) are the [`Storage`] backend's concern.
    pub fn remove(&mut self, entry: &RelPath) -> Result<(), C, S, V> {
        let blob = entry_to_blob(entry);
        if !self.storage.exists(&blob).map_err(StoreError::Storage)? {
            return Err(StoreError::NotFound(entry.as_str().to_owned()));
        }
        self.storage.remove(&blob).map_err(StoreError::Storage)?;
        self.vcs
            .commit(&[blob], &format!("bypass: Remove {entry}"))
            .map_err(StoreError::Vcs)?;
        Ok(())
    }

    /// Delete every entry under `prefix`. Returns the list of entries
    /// that were removed. If `prefix` contains no entries, returns
    /// `NotFound` so the CLI does not silently succeed on a typo.
    pub fn remove_recursive(&mut self, prefix: &RelPath) -> Result<Vec<RelPath>, C, S, V> {
        let entries = self.list(Some(prefix))?;
        if entries.is_empty() {
            return Err(StoreError::NotFound(prefix.as_str().to_owned()));
        }
        let mut blobs = Vec::with_capacity(entries.len());
        for entry in &entries {
            let blob = entry_to_blob(entry);
            self.storage.remove(&blob).map_err(StoreError::Storage)?;
            blobs.push(blob);
        }
        self.vcs
            .commit(&blobs, &format!("bypass: Remove {prefix}/"))
            .map_err(StoreError::Vcs)?;
        Ok(entries)
    }
}

/// Translate a logical entry path (`email/work`) to its on-disk blob
/// path (`email/work.gpg`).
pub(crate) fn entry_to_blob(entry: &RelPath) -> RelPath {
    RelPath::new(format!("{}.gpg", entry.as_str()))
        .expect("appending `.gpg` to a valid RelPath yields a valid RelPath")
}

/// Reverse of [`entry_to_blob`]. Returns `None` for files that are not
/// entries (no `.gpg` suffix, e.g. `.gpg-id`).
pub(crate) fn blob_to_entry(blob: &RelPath) -> Option<RelPath> {
    let stem = blob.as_str().strip_suffix(".gpg")?;
    RelPath::new(stem).ok()
}

fn gpg_id_path() -> RelPath {
    RelPath::new(".gpg-id").expect(".gpg-id is a valid RelPath")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;

    use super::*;
    use crate::vcs::NoVcs;

    // ----- MemStorage (duplicated from gpg_id::tests; kept private here
    //       so the public surface of bypass-core doesn't grow a test-only
    //       type) ---------------------------------------------------------

    #[derive(Default)]
    struct MemStorage {
        files: HashMap<String, Vec<u8>>,
    }

    impl Storage for MemStorage {
        type Error = Infallible;

        fn read(&self, path: &RelPath) -> std::result::Result<Option<Vec<u8>>, Self::Error> {
            Ok(self.files.get(path.as_str()).cloned())
        }
        fn write(&mut self, path: &RelPath, data: &[u8]) -> std::result::Result<(), Self::Error> {
            self.files.insert(path.as_str().to_owned(), data.to_vec());
            Ok(())
        }
        fn remove(&mut self, path: &RelPath) -> std::result::Result<(), Self::Error> {
            self.files.remove(path.as_str());
            Ok(())
        }
        fn exists(&self, path: &RelPath) -> std::result::Result<bool, Self::Error> {
            Ok(self.files.contains_key(path.as_str()))
        }
        fn list(&self, prefix: Option<&RelPath>) -> std::result::Result<Vec<RelPath>, Self::Error> {
            let pfx = prefix.map(|p| format!("{}/", p.as_str()));
            Ok(self
                .files
                .keys()
                .filter(|k| match &pfx {
                    Some(p) => k.starts_with(p.as_str()),
                    None => true,
                })
                .map(|s| RelPath::new(s.clone()).unwrap())
                .collect())
        }
    }

    // ----- XorCrypto: reversible "encryption" so we can roundtrip in
    //       tests without spawning gpg. recipients are recorded for
    //       inspection. ----------------------------------------------------

    struct XorCrypto;

    #[derive(Debug, thiserror::Error)]
    #[error("xor crypto error")]
    struct XorError;

    impl Crypto for XorCrypto {
        type Error = XorError;
        fn encrypt(
            &self,
            plaintext: &[u8],
            _recipients: &[KeyId],
        ) -> std::result::Result<Vec<u8>, Self::Error> {
            Ok(plaintext.iter().map(|b| b ^ 0xAA).collect())
        }
        fn decrypt(&self, ciphertext: &[u8]) -> std::result::Result<SecretBytes, Self::Error> {
            Ok(SecretBytes::new(
                ciphertext.iter().map(|b| b ^ 0xAA).collect(),
            ))
        }
    }

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn fresh_store() -> Store<XorCrypto, MemStorage, NoVcs> {
        Store::new(XorCrypto, MemStorage::default(), NoVcs)
    }

    #[test]
    fn entry_to_blob_appends_gpg() {
        assert_eq!(entry_to_blob(&rp("a")).as_str(), "a.gpg");
        assert_eq!(entry_to_blob(&rp("a/b/c")).as_str(), "a/b/c.gpg");
    }

    #[test]
    fn init_writes_gpg_id_with_recipients_one_per_line() {
        let mut store = fresh_store();
        store
            .init(&[KeyId::new("ALICE"), KeyId::new("BOB")])
            .unwrap();
        let bytes = store
            .storage
            .files
            .get(".gpg-id")
            .expect(".gpg-id was written");
        assert_eq!(bytes, b"ALICE\nBOB\n");
    }

    #[test]
    fn init_with_no_recipients_is_rejected() {
        let mut store = fresh_store();
        let err = store.init(&[]).unwrap_err();
        assert!(matches!(err, StoreError::GpgIdMalformed(_)));
    }

    #[test]
    fn insert_then_show_roundtrip() {
        let mut store = fresh_store();
        store.init(&[KeyId::new("ALICE")]).unwrap();
        store.insert(&rp("email/work"), b"hunter2", false).unwrap();
        let got = store.show(&rp("email/work")).unwrap();
        assert_eq!(got.as_slice(), b"hunter2");
    }

    #[test]
    fn insert_without_overwrite_rejects_duplicate() {
        let mut store = fresh_store();
        store.init(&[KeyId::new("ALICE")]).unwrap();
        store.insert(&rp("e"), b"v1", false).unwrap();
        let err = store.insert(&rp("e"), b"v2", false).unwrap_err();
        assert!(matches!(err, StoreError::AlreadyExists(_)));
        let still = store.show(&rp("e")).unwrap();
        assert_eq!(still.as_slice(), b"v1");
    }

    #[test]
    fn insert_with_overwrite_replaces() {
        let mut store = fresh_store();
        store.init(&[KeyId::new("ALICE")]).unwrap();
        store.insert(&rp("e"), b"v1", false).unwrap();
        store.insert(&rp("e"), b"v2", true).unwrap();
        let got = store.show(&rp("e")).unwrap();
        assert_eq!(got.as_slice(), b"v2");
    }

    #[test]
    fn show_missing_returns_not_found() {
        let mut store = fresh_store();
        store.init(&[KeyId::new("ALICE")]).unwrap();
        let err = store.show(&rp("nope")).unwrap_err();
        assert!(matches!(err, StoreError::NotFound(s) if s == "nope"));
    }

    #[test]
    fn insert_without_init_returns_not_initialized() {
        let mut store = fresh_store();
        let err = store.insert(&rp("e"), b"v", false).unwrap_err();
        assert!(matches!(err, StoreError::NotInitialized));
    }

    fn populated_store() -> Store<XorCrypto, MemStorage, NoVcs> {
        let mut store = fresh_store();
        store.init(&[KeyId::new("ALICE")]).unwrap();
        for path in ["a", "b/c", "b/d", "email/work", "email/personal"] {
            store.insert(&rp(path), b"x", false).unwrap();
        }
        store
    }

    fn entry_names(entries: &[RelPath]) -> Vec<String> {
        entries.iter().map(|e| e.as_str().to_owned()).collect()
    }

    #[test]
    fn list_all_strips_gpg_suffix_and_skips_gpg_id() {
        let store = populated_store();
        let got = entry_names(&store.list(None).unwrap());
        assert_eq!(got, vec!["a", "b/c", "b/d", "email/personal", "email/work"]);
    }

    #[test]
    fn list_with_subpath_scopes_to_subtree() {
        let store = populated_store();
        let got = entry_names(&store.list(Some(&rp("email"))).unwrap());
        assert_eq!(got, vec!["email/personal", "email/work"]);
    }

    #[test]
    fn list_of_empty_subtree_is_empty() {
        let store = populated_store();
        assert!(store.list(Some(&rp("nope"))).unwrap().is_empty());
    }

    #[test]
    fn find_substring_matches() {
        let store = populated_store();
        let got = entry_names(&store.find("email").unwrap());
        assert_eq!(got, vec!["email/personal", "email/work"]);

        let got = entry_names(&store.find("work").unwrap());
        assert_eq!(got, vec!["email/work"]);

        assert!(store.find("nomatch").unwrap().is_empty());
    }

    #[test]
    fn remove_deletes_one_entry() {
        let mut store = populated_store();
        store.remove(&rp("email/work")).unwrap();
        assert!(matches!(
            store.show(&rp("email/work")).unwrap_err(),
            StoreError::NotFound(_)
        ));
        // siblings untouched
        store.show(&rp("email/personal")).unwrap();
    }

    #[test]
    fn remove_missing_returns_not_found() {
        let mut store = populated_store();
        let err = store.remove(&rp("ghost")).unwrap_err();
        assert!(matches!(err, StoreError::NotFound(s) if s == "ghost"));
    }

    #[test]
    fn remove_recursive_clears_subtree() {
        let mut store = populated_store();
        let mut removed = entry_names(&store.remove_recursive(&rp("email")).unwrap());
        removed.sort();
        assert_eq!(removed, vec!["email/personal", "email/work"]);
        // unaffected entries remain
        let remaining = entry_names(&store.list(None).unwrap());
        assert_eq!(remaining, vec!["a", "b/c", "b/d"]);
    }

    #[test]
    fn remove_recursive_on_missing_prefix_returns_not_found() {
        let mut store = populated_store();
        let err = store.remove_recursive(&rp("nothing")).unwrap_err();
        assert!(matches!(err, StoreError::NotFound(s) if s == "nothing"));
    }

    #[test]
    fn blob_to_entry_round_trip() {
        let blob = entry_to_blob(&rp("email/work"));
        assert_eq!(blob.as_str(), "email/work.gpg");
        assert_eq!(blob_to_entry(&blob).unwrap().as_str(), "email/work");

        // non-entry files: no round-trip
        assert!(blob_to_entry(&rp(".gpg-id")).is_none());
        assert!(blob_to_entry(&rp("README")).is_none());
    }
}
