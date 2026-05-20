// SPDX-License-Identifier: GPL-3.0-or-later

//! Walk the storage tree upward from an entry path to resolve the
//! `.gpg-id` recipient list, matching `pass` semantics.
//!
//! Given an entry at `email/work/github`, this resolver looks for a
//! `.gpg-id` file at:
//!
//! ```text
//! email/work/.gpg-id
//! email/.gpg-id
//! .gpg-id          (store root)
//! ```
//!
//! and returns the recipients from the *first* one found, walking from the
//! entry outward. This is how `pass` lets a subtree override the store's
//! default recipients. Each recipient occupies its own line; blank lines
//! and lines starting with `#` are skipped.

use crate::crypto::KeyId;
use crate::path::RelPath;
use crate::storage::Storage;

/// Errors surfaced by [`resolve_recipients`].
///
/// Generic over the backing [`Storage`]'s associated error so I/O failures
/// keep their full type information all the way to the caller (see
/// [ADR-0006](../../../../doc/adr/0006-trait-associated-error-types.md)).
#[derive(Debug, thiserror::Error)]
pub enum GpgIdError<E: std::error::Error + 'static> {
    /// Walked the tree all the way to the root and never found a `.gpg-id`.
    /// The store is uninitialised (or the lookup escaped the store).
    #[error("no .gpg-id file found for this entry")]
    Missing,

    /// Found a `.gpg-id` but it contained no usable recipient lines.
    #[error(".gpg-id file is empty (no recipients)")]
    Empty,

    /// The `.gpg-id` file could not be decoded as UTF-8.
    #[error(".gpg-id is not valid UTF-8")]
    NotUtf8,

    /// Underlying storage failure.
    #[error(transparent)]
    Storage(E),
}

/// Walk from `entry_path`'s parent up to the store root, returning the
/// recipients listed in the first `.gpg-id` encountered.
pub fn resolve_recipients<S: Storage>(
    storage: &S,
    entry_path: &RelPath,
) -> Result<Vec<KeyId>, GpgIdError<S::Error>> {
    let mut current = entry_path.parent();
    loop {
        let gpg_id_path = match &current {
            Some(p) => p.join(".gpg-id").expect(".gpg-id is a valid path segment"),
            None => RelPath::new(".gpg-id").expect(".gpg-id is a valid RelPath"),
        };
        if let Some(bytes) = storage.read(&gpg_id_path).map_err(GpgIdError::Storage)? {
            return parse_gpg_id::<S::Error>(&bytes);
        }
        if current.is_none() {
            return Err(GpgIdError::Missing);
        }
        current = current.and_then(|p| p.parent());
    }
}

fn parse_gpg_id<E: std::error::Error + 'static>(bytes: &[u8]) -> Result<Vec<KeyId>, GpgIdError<E>> {
    let text = std::str::from_utf8(bytes).map_err(|_| GpgIdError::NotUtf8)?;
    let recipients: Vec<KeyId> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(KeyId::new)
        .collect();
    if recipients.is_empty() {
        return Err(GpgIdError::Empty);
    }
    Ok(recipients)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::convert::Infallible;

    use super::*;

    /// In-memory `Storage` for unit-testing core logic without touching the
    /// filesystem. `Infallible` error type — reads/writes cannot fail.
    #[derive(Default)]
    struct MemStorage {
        files: HashMap<String, Vec<u8>>,
    }

    impl MemStorage {
        fn put(&mut self, path: &str, data: &str) {
            self.files.insert(path.to_owned(), data.as_bytes().to_vec());
        }
    }

    impl Storage for MemStorage {
        type Error = Infallible;

        fn read(&self, path: &RelPath) -> Result<Option<Vec<u8>>, Self::Error> {
            Ok(self.files.get(path.as_str()).cloned())
        }

        fn write(&mut self, path: &RelPath, data: &[u8]) -> Result<(), Self::Error> {
            self.files.insert(path.as_str().to_owned(), data.to_vec());
            Ok(())
        }

        fn remove(&mut self, path: &RelPath) -> Result<(), Self::Error> {
            self.files.remove(path.as_str());
            Ok(())
        }

        fn exists(&self, path: &RelPath) -> Result<bool, Self::Error> {
            Ok(self.files.contains_key(path.as_str()))
        }

        fn list(&self, _prefix: Option<&RelPath>) -> Result<Vec<RelPath>, Self::Error> {
            Ok(self
                .files
                .keys()
                .map(|s| RelPath::new(s.clone()).unwrap())
                .collect())
        }
    }

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    #[test]
    fn finds_root_gpg_id_for_top_level_entry() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "ABCD1234\n");
        let r = resolve_recipients(&s, &rp("github")).unwrap();
        assert_eq!(r, vec![KeyId::new("ABCD1234")]);
    }

    #[test]
    fn finds_root_gpg_id_for_nested_entry() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "ABCD1234\n");
        let r = resolve_recipients(&s, &rp("email/work/github")).unwrap();
        assert_eq!(r, vec![KeyId::new("ABCD1234")]);
    }

    #[test]
    fn nearer_gpg_id_wins() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "ROOT_KEY\n");
        s.put("email/.gpg-id", "EMAIL_KEY\n");
        s.put("email/work/.gpg-id", "WORK_KEY\n");
        let r = resolve_recipients(&s, &rp("email/work/github")).unwrap();
        assert_eq!(r, vec![KeyId::new("WORK_KEY")]);
    }

    #[test]
    fn multiple_recipients_per_file() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "ALICE\nBOB\nCAROL\n");
        let r = resolve_recipients(&s, &rp("shared/db")).unwrap();
        assert_eq!(
            r,
            vec![KeyId::new("ALICE"), KeyId::new("BOB"), KeyId::new("CAROL")]
        );
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "# primary\nALICE\n\n# backup\nBOB\n");
        let r = resolve_recipients(&s, &rp("entry")).unwrap();
        assert_eq!(r, vec![KeyId::new("ALICE"), KeyId::new("BOB")]);
    }

    #[test]
    fn whitespace_around_recipients_is_trimmed() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "  ALICE  \n\tBOB\t\n");
        let r = resolve_recipients(&s, &rp("entry")).unwrap();
        assert_eq!(r, vec![KeyId::new("ALICE"), KeyId::new("BOB")]);
    }

    #[test]
    fn no_gpg_id_anywhere_returns_missing() {
        let s = MemStorage::default();
        let err = resolve_recipients(&s, &rp("a/b/c")).unwrap_err();
        assert!(matches!(err, GpgIdError::Missing));
    }

    #[test]
    fn empty_gpg_id_returns_empty() {
        let mut s = MemStorage::default();
        s.put(".gpg-id", "# only comments\n\n");
        let err = resolve_recipients(&s, &rp("entry")).unwrap_err();
        assert!(matches!(err, GpgIdError::Empty));
    }

    #[test]
    fn non_utf8_gpg_id_is_rejected() {
        let mut s = MemStorage::default();
        s.files.insert(".gpg-id".to_owned(), vec![0xff, 0xfe, 0xfd]);
        let err = resolve_recipients(&s, &rp("entry")).unwrap_err();
        assert!(matches!(err, GpgIdError::NotUtf8));
    }
}
