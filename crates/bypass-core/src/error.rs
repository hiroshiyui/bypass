// SPDX-License-Identifier: GPL-3.0-or-later

//! Core error type — only failures that this crate itself can produce.
//!
//! Concrete I/O / crypto / VCS errors live on the respective trait impls'
//! associated `Error` types (see [`crate::crypto::Crypto::Error`],
//! [`crate::storage::Storage::Error`], [`crate::vcs::VersionControl::Error`]).
//! The [`crate::store`] orchestrator will wrap those into a higher-level
//! error type when it lands.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid store path: {0}")]
    InvalidPath(String),

    #[error("entry not found: {0}")]
    NotFound(String),

    #[error("entry already exists: {0}")]
    AlreadyExists(String),

    #[error("no .gpg-id file found for entry")]
    MissingGpgId,

    #[error("malformed entry: {0}")]
    MalformedEntry(String),
}

pub type Result<T> = std::result::Result<T, Error>;
