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
}

/// Translate a logical entry path (`email/work`) to its on-disk blob
/// path (`email/work.gpg`).
pub(crate) fn entry_to_blob(entry: &RelPath) -> RelPath {
    RelPath::new(format!("{}.gpg", entry.as_str()))
        .expect("appending `.gpg` to a valid RelPath yields a valid RelPath")
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
        fn list(
            &self,
            _prefix: Option<&RelPath>,
        ) -> std::result::Result<Vec<RelPath>, Self::Error> {
            Ok(self
                .files
                .keys()
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
}
