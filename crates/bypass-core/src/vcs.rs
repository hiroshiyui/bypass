// SPDX-License-Identifier: GPL-3.0-or-later

//! Optional version-control abstraction. Linux CLI backs this with `git2`;
//! browser/Android may opt out entirely and rely on external sync.
//!
//! The trait describes the small set of operations the password-store
//! workflow needs (init the repo, stage + commit a known set of paths,
//! enumerate history for an entry). Anything richer — branches, remotes,
//! merges — is the implementation's concern, not the core's.

use crate::path::RelPath;

/// A single point in the store's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// Opaque commit identifier (e.g. a git SHA, as a hex string).
    pub id: String,
    /// Short, human-readable summary.
    pub summary: String,
    /// Author name, if known.
    pub author: Option<String>,
    /// Commit time, as a unix timestamp in seconds.
    pub time_unix: i64,
}

/// Repository-style versioning of a [`crate::storage::Storage`].
///
/// Implementations must address the same on-disk layout the Storage trait
/// exposes; the [`crate::store`] orchestrator drives both in lockstep and
/// will call [`commit`](Self::commit) with the exact set of paths it just
/// mutated.
pub trait VersionControl {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Create an empty repository at the store root. No-op if one already
    /// exists.
    fn init(&mut self) -> Result<(), Self::Error>;

    /// Whether a repository is present.
    fn is_initialized(&self) -> Result<bool, Self::Error>;

    /// Stage and commit the given paths with `message`. Implementations
    /// must be a no-op (returning `Ok`) if `paths` is empty or if none of
    /// the listed paths have changed.
    fn commit(&mut self, paths: &[RelPath], message: &str) -> Result<(), Self::Error>;

    /// Commit history, newest first. When `path` is `Some(p)` only commits
    /// whose diff touches `p` (or anything under `p/`) are returned; when
    /// `None`, every commit reachable from `HEAD` is returned. An unbounded
    /// log is acceptable for now; pagination can be added later if needed.
    fn log(&self, path: Option<&RelPath>) -> Result<Vec<Commit>, Self::Error>;
}

/// A no-op [`VersionControl`] for stores that should not be versioned.
///
/// Frontends construct `Store::new(crypto, storage, NoVcs)` when the user
/// has disabled git, when the platform has no filesystem, or — during
/// Phase 1 — while the real `Git2Vcs` impl is still pending. Every method
/// succeeds without doing anything; `is_initialized` reports `false`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoVcs;

impl VersionControl for NoVcs {
    type Error = std::convert::Infallible;

    fn init(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn is_initialized(&self) -> Result<bool, Self::Error> {
        Ok(false)
    }

    fn commit(&mut self, _paths: &[RelPath], _message: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn log(&self, _path: Option<&RelPath>) -> Result<Vec<Commit>, Self::Error> {
        Ok(Vec::new())
    }
}
