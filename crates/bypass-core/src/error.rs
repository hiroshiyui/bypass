// SPDX-License-Identifier: GPL-3.0-or-later

//! Core error type. Concrete I/O errors from frontend implementations are
//! carried as boxed sources so this crate stays free of platform deps.

use std::io;

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

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("version-control error: {0}")]
    Vcs(String),

    #[error("malformed entry: {0}")]
    MalformedEntry(String),

    #[error(transparent)]
    Io(#[from] io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
