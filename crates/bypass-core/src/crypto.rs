// SPDX-License-Identifier: GPL-3.0-or-later

//! Crypto abstraction. Implemented per platform: `gpg` subprocess on the
//! CLI, OpenKeychain (via callback interface) on Android, native-messaging
//! host relay in the browser extension. Core never speaks OpenPGP itself.
//!
//! This module defines:
//!
//! - [`SecretBytes`]: a zeroize-on-drop buffer for plaintext secrets.
//! - [`KeyId`]: an OpenPGP recipient identifier (long key id, fingerprint,
//!   or user-id — interpretation is up to the impl).
//! - [`Crypto`]: the trait every frontend implements.

use std::fmt;

use zeroize::{Zeroize, ZeroizeOnDrop};

/// A buffer of plaintext bytes that is zeroized when dropped.
///
/// `Debug` deliberately does not print the contents — only the length —
/// so accidental `{:?}` formatting cannot leak secrets into logs.
#[derive(Zeroize, ZeroizeOnDrop, Clone, PartialEq, Eq)]
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<u8>> for SecretBytes {
    fn from(v: Vec<u8>) -> Self {
        Self::new(v)
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretBytes")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// An OpenPGP recipient identifier. Interpretation (long key id,
/// fingerprint, user id, …) is up to the [`Crypto`] implementation —
/// the core just passes the string through.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyId(String);

impl KeyId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for KeyId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for KeyId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl fmt::Display for KeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// OpenPGP encrypt/decrypt provider. One implementation per frontend.
///
/// The trait is intentionally minimal: signing, key listing, and trust
/// management are out of scope. Recipients are addressed by [`KeyId`]
/// and ciphertext is opaque bytes — the core does not inspect packets.
pub trait Crypto {
    /// Errors raised by this implementation. Frontends typically pick a
    /// rich, domain-specific type (e.g. one that captures `gpg` exit codes
    /// and stderr); the core only requires it be `std::error::Error`.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Encrypt `plaintext` to the given recipients.
    fn encrypt(&self, plaintext: &[u8], recipients: &[KeyId]) -> Result<Vec<u8>, Self::Error>;

    /// Decrypt `ciphertext` to a zeroize-on-drop buffer.
    fn decrypt(&self, ciphertext: &[u8]) -> Result<SecretBytes, Self::Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_bytes_debug_hides_contents() {
        let s = SecretBytes::new(b"hunter2".to_vec());
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2"), "Debug leaked contents: {dbg}");
        assert!(dbg.contains("len"));
    }

    #[test]
    fn secret_bytes_as_slice_roundtrip() {
        let s = SecretBytes::new(vec![1, 2, 3]);
        assert_eq!(s.as_slice(), &[1, 2, 3]);
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
    }

    #[test]
    fn key_id_constructors() {
        assert_eq!(KeyId::from("abcd").as_str(), "abcd");
        assert_eq!(KeyId::new("abcd".to_owned()).as_str(), "abcd");
        assert_eq!(format!("{}", KeyId::new("abcd")), "abcd");
    }
}
