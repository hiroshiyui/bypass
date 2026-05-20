// SPDX-License-Identifier: GPL-3.0-or-later

//! Storage abstraction over an opaque blob namespace, keyed by
//! [`RelPath`](crate::path::RelPath).
//!
//! Frontends back this with whatever they have: the CLI uses the local
//! filesystem under `~/.password-store`; Android uses app-scoped storage;
//! the browser extension proxies to the desktop binary. The core only
//! reads/writes raw bytes — it does not know or care that the blobs happen
//! to be OpenPGP ciphertext.
//!
//! Pass-compatibility note: the on-disk layout (`<name>.gpg`, `.gpg-id`)
//! is a *convention enforced by the [`crate::store`] orchestrator*, not by
//! this trait. The `.gpg` suffix is part of the `RelPath` callers pass in.

use crate::path::RelPath;

/// A blob store addressed by [`RelPath`].
pub trait Storage {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Read the blob at `path`. Returns `Ok(None)` if it does not exist;
    /// returns `Err` only for genuine I/O failures.
    fn read(&self, path: &RelPath) -> Result<Option<Vec<u8>>, Self::Error>;

    /// Write `data` to `path`, creating intermediate directories as needed
    /// and overwriting any existing blob.
    fn write(&mut self, path: &RelPath, data: &[u8]) -> Result<(), Self::Error>;

    /// Remove the blob at `path`. No-op if it does not exist.
    fn remove(&mut self, path: &RelPath) -> Result<(), Self::Error>;

    /// Whether a blob exists at `path`.
    fn exists(&self, path: &RelPath) -> Result<bool, Self::Error>;

    /// Recursively enumerate blob paths underneath `prefix`. Pass `None` to
    /// list the entire store. The order of returned paths is unspecified.
    fn list(&self, prefix: Option<&RelPath>) -> Result<Vec<RelPath>, Self::Error>;
}
