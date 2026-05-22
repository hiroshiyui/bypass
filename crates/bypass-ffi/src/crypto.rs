// SPDX-License-Identifier: GPL-3.0-or-later

//! Bridge between
//! [`bypass_core::crypto::Crypto`](bypass_core::crypto::Crypto) and the
//! UniFFI **callback interface** the Kotlin side implements. The
//! Android app supplies an `OpenKeychainCrypto` (8.2) that talks to
//! OpenKeychain's OpenPGP AIDL service; Rust calls it through this
//! adapter. See
//! [ADR-0024](../../../../doc/adr/0024-android-ffi-via-uniffi.md)
//! §"Crypto direction".

use std::sync::Arc;

use bypass_core::crypto::{KeyId, SecretBytes};

use crate::error::BypassError;

/// Foreign-implemented trait. Kotlin code declares an
/// `implements Crypto` class; UniFFI's bindgen produces the JNI
/// shims that let Rust invoke its methods.
///
/// Both methods can fail (network drop, key unavailable, user
/// cancelled the OpenKeychain prompt); UniFFI marshals the
/// `BypassError` back across the FFI as a Kotlin exception on the
/// `BypassException.Crypto` branch.
#[uniffi::export(with_foreign)]
pub trait Crypto: Send + Sync {
    fn encrypt(&self, plaintext: Vec<u8>, recipients: Vec<String>) -> Result<Vec<u8>, BypassError>;
    fn decrypt(&self, ciphertext: Vec<u8>) -> Result<Vec<u8>, BypassError>;
}

/// Adapter that lets the core's
/// [`Crypto`](bypass_core::crypto::Crypto) trait call into the
/// UniFFI callback above.
///
/// The `Arc<dyn Crypto>` is supplied at `BypassStore::open` time and
/// stored for the lifetime of the store. Each `encrypt` / `decrypt`
/// allocates Vecs at the FFI boundary — necessary because the JVM
/// needs owned `ByteArray`s — but those copies live only as long as
/// the call.
pub(crate) struct CryptoCallback {
    inner: Arc<dyn Crypto>,
}

impl CryptoCallback {
    pub(crate) fn new(inner: Arc<dyn Crypto>) -> Self {
        Self { inner }
    }
}

impl bypass_core::crypto::Crypto for CryptoCallback {
    type Error = BypassError;

    fn encrypt(&self, plaintext: &[u8], recipients: &[KeyId]) -> Result<Vec<u8>, Self::Error> {
        let recipient_strs: Vec<String> =
            recipients.iter().map(|r| r.as_str().to_owned()).collect();
        // Copy the plaintext for the FFI call. The JVM-side
        // `ByteArray` owns its own buffer; we cannot share by
        // reference across the boundary.
        self.inner.encrypt(plaintext.to_vec(), recipient_strs)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretBytes, Self::Error> {
        let plain = self.inner.decrypt(ciphertext.to_vec())?;
        // Wrap the JVM-supplied bytes in `SecretBytes` so they
        // zeroize on drop while they live in Rust. The JVM's
        // own `ByteArray` is the caller's problem — JS-side
        // / JVM-side string immutability is documented in
        // ADR-0024 §"Plaintext on Android".
        Ok(SecretBytes::new(plain))
    }
}
