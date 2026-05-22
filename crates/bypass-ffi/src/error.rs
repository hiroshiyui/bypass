// SPDX-License-Identifier: GPL-3.0-or-later

//! Single user-facing error enum exposed across the UniFFI
//! boundary. Maps to a Kotlin sealed class via
//! `#[derive(uniffi::Error)]`. See
//! [ADR-0024](../../../../doc/adr/0024-android-ffi-via-uniffi.md)
//! §"Error mapping" for the rationale.
//!
//! Internally `bypass_core` returns
//! [`StoreError<C, S, V>`](bypass_core::store::StoreError) — generic
//! over the concrete Crypto / Storage / VCS error types — but UniFFI
//! cannot represent generics across the FFI. We flatten on the way
//! out via `From` impls.

use bypass_core::store::StoreError;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum BypassError {
    #[error("entry not found: {path}")]
    NotFound { path: String },

    #[error("entry already exists: {path}")]
    AlreadyExists { path: String },

    #[error("invalid entry path: {message}")]
    InvalidPath { message: String },

    #[error("store not initialised; call `init` first")]
    NotInitialized,

    #[error("malformed .gpg-id: {message}")]
    GpgIdMalformed { message: String },

    #[error("crypto error: {message}")]
    Crypto { message: String },

    #[error("storage error: {message}")]
    Storage { message: String },

    #[error("internal error: {message}")]
    Internal { message: String },
}

impl<CE, SE, VE> From<StoreError<CE, SE, VE>> for BypassError
where
    CE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    VE: std::error::Error + Send + Sync + 'static,
{
    fn from(e: StoreError<CE, SE, VE>) -> Self {
        match e {
            StoreError::NotFound(path) => Self::NotFound { path },
            StoreError::AlreadyExists(path) => Self::AlreadyExists { path },
            StoreError::NotInitialized => Self::NotInitialized,
            StoreError::GpgIdMalformed(m) => Self::GpgIdMalformed {
                message: m.to_owned(),
            },
            StoreError::Crypto(source) => Self::Crypto {
                message: source.to_string(),
            },
            StoreError::Storage(source) => Self::Storage {
                message: source.to_string(),
            },
            StoreError::Vcs(source) => Self::Internal {
                message: format!("vcs: {source}"),
            },
        }
    }
}

impl From<bypass_core::error::Error> for BypassError {
    fn from(e: bypass_core::error::Error) -> Self {
        Self::InvalidPath {
            message: e.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_is_user_friendly() {
        let e = BypassError::NotFound {
            path: "email/work".into(),
        };
        assert_eq!(e.to_string(), "entry not found: email/work");
    }

    #[test]
    fn from_store_error_not_found() {
        // We need concrete error types to construct a StoreError. Use
        // anyhow::Error::new(io::Error...) wrapped — easiest path is
        // the StoreError::NotFound variant which doesn't need any of
        // the generic error sources.
        let e: StoreError<std::io::Error, std::io::Error, std::io::Error> =
            StoreError::NotFound("a/b".into());
        let mapped: BypassError = e.into();
        assert!(matches!(mapped, BypassError::NotFound { path } if path == "a/b"));
    }

    #[test]
    fn from_store_error_not_initialized() {
        let e: StoreError<std::io::Error, std::io::Error, std::io::Error> =
            StoreError::NotInitialized;
        let mapped: BypassError = e.into();
        assert!(matches!(mapped, BypassError::NotInitialized));
    }

    #[test]
    fn from_invalid_path() {
        let e = bypass_core::error::Error::InvalidPath("contains NUL".into());
        let mapped: BypassError = e.into();
        assert!(matches!(mapped, BypassError::InvalidPath { message } if message.contains("NUL")));
    }
}
