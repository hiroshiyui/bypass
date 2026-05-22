// SPDX-License-Identifier: GPL-3.0-or-later

//! UniFFI-exported surface for `bypass-core`, designed for the
//! Android app (Phase 8) and a possible future iOS port.
//!
//! See [ADR-0024](../../../doc/adr/0024-android-ffi-via-uniffi.md).
//! Architecture in one paragraph: `BypassStore` is a UniFFI object
//! that wraps `bypass_core::store::Store<CryptoCallback, AppStorage,
//! NoVcs>`. Kotlin opens it once at app start, then drives every
//! CRUD op through methods on the object. `Crypto` is a UniFFI
//! callback interface — Kotlin implements it (via OpenKeychain in
//! 8.2); Rust calls into Kotlin for `encrypt` / `decrypt`. Errors
//! flatten to a single `BypassError` enum
//! (see [`error`]).

use std::sync::{Arc, Mutex};

use bypass_core::crypto::KeyId;
use bypass_core::path::RelPath;
use bypass_core::store::Store;
use bypass_core::vcs::NoVcs;

pub mod crypto;
pub mod error;
mod storage;

pub use crate::crypto::Crypto;
pub use crate::error::BypassError;

use crate::crypto::CryptoCallback;
use crate::storage::AppStorage;

uniffi::setup_scaffolding!("bypass");

/// Concrete `Store` for the Android target.
type AppStore = Store<CryptoCallback, AppStorage, NoVcs>;

/// UniFFI object holding the password store. One instance per
/// app process (constructed at app start, dropped on shutdown).
///
/// All methods take `&self`; mutation goes through the inner
/// `Mutex`. The mutex makes the store safe to call from multiple
/// threads (Compose UI dispatches I/O to a coroutine; UniFFI's
/// blocking calls happen off the main thread). It is **not** a
/// performance bottleneck because individual store ops are I/O-
/// bound on the GPG callback round-trip anyway.
#[derive(uniffi::Object)]
pub struct BypassStore {
    inner: Mutex<AppStore>,
}

#[uniffi::export]
impl BypassStore {
    /// Open the store rooted at `root_dir`. `crypto` is the
    /// Kotlin-side OpenKeychain client (or any other `Crypto`
    /// implementation).
    ///
    /// `root_dir` is typically `context.filesDir.resolve("store")
    /// .absolutePath` on Android.
    #[uniffi::constructor]
    pub fn open(root_dir: String, crypto: Arc<dyn Crypto>) -> Arc<Self> {
        let storage = AppStorage::new(std::path::PathBuf::from(root_dir));
        let crypto_wrap = CryptoCallback::new(crypto);
        let inner = Store::new(crypto_wrap, storage, NoVcs);
        Arc::new(Self {
            inner: Mutex::new(inner),
        })
    }

    /// Initialise the store with one or more OpenPGP recipient
    /// identifiers (typically key fingerprints or email addresses
    /// that resolve to keys via OpenKeychain).
    pub fn init(&self, recipients: Vec<String>) -> Result<(), BypassError> {
        let keys: Vec<KeyId> = recipients.into_iter().map(KeyId::new).collect();
        self.lock()?.init(&keys).map_err(Into::into)
    }

    /// Encrypt `plaintext` to the recipients in `.gpg-id` and write
    /// the ciphertext to the blob at `path.gpg`.
    pub fn insert(
        &self,
        path: String,
        plaintext: Vec<u8>,
        overwrite: bool,
    ) -> Result<(), BypassError> {
        let entry = parse_path(&path)?;
        self.lock()?
            .insert(&entry, &plaintext, overwrite)
            .map_err(Into::into)
    }

    /// Decrypt the entry at `path` and return its plaintext bytes.
    ///
    /// **Plaintext lifetime warning**: the JVM-side `ByteArray`
    /// holds the decrypted bytes. JVM arrays are not zeroizable from
    /// Rust; callers should drop the reference promptly. See
    /// ADR-0024 §"Plaintext on Android".
    pub fn show(&self, path: String) -> Result<Vec<u8>, BypassError> {
        let entry = parse_path(&path)?;
        let plaintext = self.lock()?.show(&entry)?;
        // SecretBytes drops with zeroize after this clone; the
        // returned Vec is the JVM's problem from here.
        Ok(plaintext.as_slice().to_vec())
    }

    /// Decrypt the entry at `path` and return one field's value.
    /// Field matching is case-insensitive. Returns
    /// `BypassError::NotFound` if the entry has no such field.
    pub fn show_field(&self, path: String, field: String) -> Result<String, BypassError> {
        let entry = parse_path(&path)?;
        let plaintext = self.lock()?.show(&entry)?;
        let parsed = bypass_core::entry::Entry::parse(plaintext.as_slice()).map_err(|e| {
            BypassError::Internal {
                message: format!("parse entry body: {e}"),
            }
        })?;
        parsed
            .field(&field)
            .map(str::to_owned)
            .ok_or_else(|| BypassError::NotFound {
                path: format!("{path}#{field}"),
            })
    }

    /// All entry names under `subpath` (or below the store root
    /// when `null`), sorted lexicographically. Entry paths don't
    /// carry their `.gpg` suffix.
    pub fn ls(&self, subpath: Option<String>) -> Result<Vec<String>, BypassError> {
        let sub = subpath.as_deref().map(parse_path).transpose()?;
        let entries = self.lock()?.list(sub.as_ref())?;
        Ok(entries.into_iter().map(|p| p.as_str().to_owned()).collect())
    }

    /// All entries whose path contains `pattern` as a substring.
    pub fn find(&self, pattern: String) -> Result<Vec<String>, BypassError> {
        let entries = self.lock()?.find(&pattern)?;
        Ok(entries.into_iter().map(|p| p.as_str().to_owned()).collect())
    }

    /// Generate a random password (using the OS CSPRNG), insert it,
    /// and return it.
    ///
    /// - `length` defaults to 25.
    /// - `symbols=false` produces an alphanumeric-only password.
    /// - `in_place=true` replaces only the first line of an
    ///   existing entry, preserving the body (useful for rotation
    ///   without losing metadata like a TOTP secret).
    /// - `force=true` allows overwriting an existing entry; ignored
    ///   when `in_place=true`.
    pub fn generate(
        &self,
        path: String,
        length: Option<u32>,
        symbols: Option<bool>,
        in_place: bool,
        force: bool,
    ) -> Result<String, BypassError> {
        let entry = parse_path(&path)?;
        let length = length
            .map(|n| n as usize)
            .unwrap_or(bypass_core::generate::DEFAULT_LENGTH);
        let with_symbols = symbols.unwrap_or(true);
        let password = bypass_core::generate::generate(length, with_symbols);
        let mut store = self.lock()?;
        if in_place {
            let existing = store
                .show(&entry)
                .map(|b| b.as_slice().to_vec())
                .map_err(BypassError::from)?;
            let tail: &[u8] = match existing.iter().position(|&b| b == b'\n') {
                Some(i) => &existing[i..],
                None => b"",
            };
            let mut new_body = password.as_bytes().to_vec();
            new_body.extend_from_slice(tail);
            store.insert(&entry, &new_body, /*overwrite=*/ true)?;
        } else {
            store.insert(&entry, password.as_bytes(), force)?;
        }
        Ok(password)
    }

    /// Compute the current TOTP code for the entry at `path`. The
    /// entry must contain an `otpauth://` URI line; field-style
    /// parsing matches `pass-otp`.
    pub fn otp(&self, path: String) -> Result<String, BypassError> {
        let entry = parse_path(&path)?;
        let plaintext = self.lock()?.show(&entry)?;
        let text =
            std::str::from_utf8(plaintext.as_slice()).map_err(|_| BypassError::Internal {
                message: "entry is not valid UTF-8".into(),
            })?;
        bypass_core::otp::current_code(text).map_err(|e| BypassError::Internal {
            message: format!("compute TOTP code: {e}"),
        })
    }

    /// Remove an entry. With `recursive=true`, removes every entry
    /// under the path (and returns `NotFound` if the subtree is
    /// empty).
    pub fn rm(&self, path: String, recursive: bool) -> Result<(), BypassError> {
        let entry = parse_path(&path)?;
        let mut store = self.lock()?;
        if recursive {
            store.remove_recursive(&entry).map(|_| ())
        } else {
            store.remove(&entry)
        }
        .map_err(Into::into)
    }
}

impl BypassStore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, AppStore>, BypassError> {
        // PoisonError surfaces if a previous panic inside the store
        // tore the mutex. Treat as Internal — the UI can offer
        // "restart the app" since recovery isn't safe.
        self.inner.lock().map_err(|_| BypassError::Internal {
            message: "store mutex poisoned by a prior panic".into(),
        })
    }
}

fn parse_path(s: &str) -> Result<RelPath, BypassError> {
    RelPath::new(s).map_err(|e| BypassError::InvalidPath {
        message: e.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bypass_core::crypto::SecretBytes;

    /// Reversible "XOR with 0xAA" fake Crypto so tests don't need a
    /// real OpenPGP backend. Mirrors the TaggedCrypto pattern in
    /// `bypass_core::store::tests`.
    struct TaggedCrypto;

    impl Crypto for TaggedCrypto {
        fn encrypt(
            &self,
            plaintext: Vec<u8>,
            recipients: Vec<String>,
        ) -> Result<Vec<u8>, BypassError> {
            let header = recipients.join(",");
            let mut out: Vec<u8> = header.into_bytes();
            out.push(b'|');
            out.extend(plaintext.iter().map(|b| b ^ 0xAA));
            Ok(out)
        }
        fn decrypt(&self, ciphertext: Vec<u8>) -> Result<Vec<u8>, BypassError> {
            let pos =
                ciphertext
                    .iter()
                    .position(|&b| b == b'|')
                    .ok_or_else(|| BypassError::Crypto {
                        message: "missing header separator".into(),
                    })?;
            Ok(ciphertext[pos + 1..].iter().map(|b| b ^ 0xAA).collect())
        }
    }

    fn open_test_store() -> (tempfile::TempDir, Arc<BypassStore>) {
        let td = tempfile::TempDir::new().unwrap();
        let store = BypassStore::open(
            td.path().to_string_lossy().into_owned(),
            Arc::new(TaggedCrypto),
        );
        store.init(vec!["ALICE".into()]).unwrap();
        (td, store)
    }

    #[test]
    fn full_crud_round_trip_through_the_ffi_surface() {
        let (_td, store) = open_test_store();
        store
            .insert("email/work".into(), b"hunter2".to_vec(), false)
            .unwrap();
        let plaintext = store.show("email/work".into()).unwrap();
        assert_eq!(plaintext, b"hunter2");
        let entries = store.ls(None).unwrap();
        assert!(entries.contains(&"email/work".to_owned()));
        store.rm("email/work".into(), false).unwrap();
        let err = store.show("email/work".into()).unwrap_err();
        assert!(matches!(err, BypassError::NotFound { .. }));
    }

    #[test]
    fn show_field_extracts_named_value() {
        let (_td, store) = open_test_store();
        let body = b"hunter2\nlogin: alice\nurl: https://example.com\n".to_vec();
        store.insert("service".into(), body, false).unwrap();
        let login = store.show_field("service".into(), "login".into()).unwrap();
        assert_eq!(login, "alice");
    }

    #[test]
    fn show_field_missing_field_is_not_found() {
        let (_td, store) = open_test_store();
        store
            .insert("entry".into(), b"hunter2".to_vec(), false)
            .unwrap();
        let err = store
            .show_field("entry".into(), "nonexistent".into())
            .unwrap_err();
        assert!(matches!(err, BypassError::NotFound { .. }));
    }

    #[test]
    fn invalid_path_string_returns_invalid_path_error() {
        let (_td, store) = open_test_store();
        let err = store.show("../escape".into()).unwrap_err();
        assert!(matches!(err, BypassError::InvalidPath { .. }));
    }

    #[test]
    fn generate_writes_and_returns_the_password() {
        let (_td, store) = open_test_store();
        let pw = store
            .generate("auto".into(), Some(16), Some(false), false, false)
            .unwrap();
        assert_eq!(pw.chars().count(), 16);
        let stored = store.show("auto".into()).unwrap();
        assert_eq!(stored, pw.as_bytes());
    }

    #[test]
    fn generate_in_place_preserves_tail() {
        let (_td, store) = open_test_store();
        store
            .insert("service".into(), b"oldpw\nlogin: alice\n".to_vec(), false)
            .unwrap();
        let new_pw = store
            .generate(
                "service".into(),
                Some(8),
                Some(false),
                /*in_place=*/ true,
                false,
            )
            .unwrap();
        let body = store.show("service".into()).unwrap();
        // First line replaced; tail preserved.
        assert!(body.starts_with(new_pw.as_bytes()));
        assert!(
            body.windows(b"login: alice".len())
                .any(|w| w == b"login: alice")
        );
    }

    #[test]
    fn find_matches_substring() {
        let (_td, store) = open_test_store();
        store
            .insert("email/personal".into(), b"x".to_vec(), false)
            .unwrap();
        store
            .insert("email/work".into(), b"x".to_vec(), false)
            .unwrap();
        store
            .insert("bank/visa".into(), b"x".to_vec(), false)
            .unwrap();
        let mut matches = store.find("email".into()).unwrap();
        matches.sort();
        assert_eq!(matches, vec!["email/personal", "email/work"]);
    }

    #[test]
    fn rm_recursive_clears_subtree() {
        let (_td, store) = open_test_store();
        store
            .insert("email/a".into(), b"x".to_vec(), false)
            .unwrap();
        store
            .insert("email/b".into(), b"x".to_vec(), false)
            .unwrap();
        store.rm("email".into(), /*recursive=*/ true).unwrap();
        let entries = store.ls(None).unwrap();
        assert!(entries.iter().all(|e| !e.starts_with("email/")));
    }

    #[test]
    fn already_exists_is_surfaced() {
        let (_td, store) = open_test_store();
        store.insert("dup".into(), b"v1".to_vec(), false).unwrap();
        let err = store
            .insert("dup".into(), b"v2".to_vec(), false)
            .unwrap_err();
        assert!(matches!(err, BypassError::AlreadyExists { .. }));
    }

    #[test]
    fn crypto_callback_decrypt_returns_secret_bytes_in_core() {
        // Whitebox check: the CryptoCallback wraps the FFI bytes in
        // SecretBytes, so decrypted plaintext zeroizes on drop.
        let inner: Arc<dyn Crypto> = Arc::new(TaggedCrypto);
        let cb = crate::crypto::CryptoCallback::new(inner);
        let ct = <crate::crypto::CryptoCallback as bypass_core::crypto::Crypto>::encrypt(
            &cb,
            b"sekret",
            &[KeyId::new("X")],
        )
        .unwrap();
        let pt: SecretBytes =
            <crate::crypto::CryptoCallback as bypass_core::crypto::Crypto>::decrypt(&cb, &ct)
                .unwrap();
        assert_eq!(pt.as_slice(), b"sekret");
    }
}
