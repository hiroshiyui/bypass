// SPDX-License-Identifier: GPL-3.0-or-later

//! Traversal-safe relative paths inside the store.
//!
//! A [`RelPath`] is the canonical address of an entry (or any blob) inside a
//! [`crate::storage::Storage`]. It is intentionally stricter than
//! [`std::path::Path`]: the goal is that a hostile or buggy caller cannot
//! escape the store root, address devices, or smuggle control bytes into
//! filesystem APIs. Frontend implementations are expected to translate a
//! `RelPath` into a platform-native path by joining it onto a known-good
//! root — the segments themselves are guaranteed to be safe to append.

use std::fmt;

use crate::error::Error;

/// A non-empty, forward-slash-separated path relative to the store root.
///
/// Invariants enforced at construction:
///
/// - non-empty
/// - no leading or trailing `/`
/// - no empty segments (no `//`)
/// - no `.` or `..` segments
/// - no NUL bytes
/// - no backslashes (paths are POSIX-style, even on Windows hosts)
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RelPath(String);

impl RelPath {
    /// Construct a `RelPath`, validating the invariants above.
    pub fn new(s: impl Into<String>) -> Result<Self, Error> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(Self(s))
    }

    fn validate(s: &str) -> Result<(), Error> {
        if s.is_empty() {
            return Err(Error::InvalidPath("path is empty".into()));
        }
        if s.starts_with('/') {
            return Err(Error::InvalidPath(format!("path is absolute: {s:?}")));
        }
        if s.ends_with('/') {
            return Err(Error::InvalidPath(format!(
                "path has trailing slash: {s:?}"
            )));
        }
        if s.contains('\0') {
            return Err(Error::InvalidPath(format!("path contains NUL: {s:?}")));
        }
        if s.contains('\\') {
            return Err(Error::InvalidPath(format!(
                "path contains backslash: {s:?}"
            )));
        }
        for segment in s.split('/') {
            match segment {
                "" => {
                    return Err(Error::InvalidPath(format!("path has empty segment: {s:?}")));
                }
                "." | ".." => {
                    return Err(Error::InvalidPath(format!(
                        "path has `{segment}` segment: {s:?}"
                    )));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The path as a string slice (always valid UTF-8, never empty).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Iterate over the path's segments, in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }

    /// The parent path, or `None` if this is a top-level entry.
    pub fn parent(&self) -> Option<RelPath> {
        let (head, _) = self.0.rsplit_once('/')?;
        Some(RelPath(head.to_owned()))
    }

    /// The final segment of the path (the "file name").
    pub fn file_name(&self) -> &str {
        match self.0.rsplit_once('/') {
            Some((_, tail)) => tail,
            None => &self.0,
        }
    }

    /// Return a new `RelPath` with `segment` appended.
    ///
    /// The segment is validated the same way as a full path, plus it must not
    /// itself contain a `/`.
    pub fn join(&self, segment: &str) -> Result<RelPath, Error> {
        if segment.contains('/') {
            return Err(Error::InvalidPath(format!(
                "join segment contains `/`: {segment:?}"
            )));
        }
        Self::new(format!("{}/{}", self.0, segment))
    }
}

impl fmt::Debug for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RelPath({:?})", self.0)
    }
}

impl fmt::Display for RelPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for RelPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_simple_paths() {
        for s in ["email", "email/work", "a/b/c", "with space", "weird-name_1"] {
            assert!(RelPath::new(s).is_ok(), "{s:?} should be accepted");
        }
    }

    #[test]
    fn rejects_empty() {
        assert!(matches!(RelPath::new(""), Err(Error::InvalidPath(_))));
    }

    #[test]
    fn rejects_absolute() {
        assert!(matches!(
            RelPath::new("/etc/passwd"),
            Err(Error::InvalidPath(_))
        ));
    }

    #[test]
    fn rejects_trailing_slash() {
        assert!(matches!(RelPath::new("email/"), Err(Error::InvalidPath(_))));
    }

    #[test]
    fn rejects_dot_segments() {
        for s in ["./email", "email/./work", "email/.", "."] {
            assert!(
                matches!(RelPath::new(s), Err(Error::InvalidPath(_))),
                "{s:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_dotdot_segments() {
        for s in ["..", "../etc", "email/../passwd", "email/.."] {
            assert!(
                matches!(RelPath::new(s), Err(Error::InvalidPath(_))),
                "{s:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_empty_segments() {
        assert!(matches!(RelPath::new("a//b"), Err(Error::InvalidPath(_))));
    }

    #[test]
    fn rejects_nul() {
        assert!(matches!(
            RelPath::new("ema\0il"),
            Err(Error::InvalidPath(_))
        ));
    }

    #[test]
    fn rejects_backslash() {
        assert!(matches!(
            RelPath::new("email\\work"),
            Err(Error::InvalidPath(_))
        ));
    }

    #[test]
    fn parent_and_file_name() {
        let p = RelPath::new("email/work/github").unwrap();
        assert_eq!(p.file_name(), "github");
        let parent = p.parent().unwrap();
        assert_eq!(parent.as_str(), "email/work");
        assert_eq!(parent.parent().unwrap().as_str(), "email");
        assert!(parent.parent().unwrap().parent().is_none());
    }

    #[test]
    fn join_appends_segment() {
        let p = RelPath::new("email").unwrap();
        assert_eq!(p.join("work").unwrap().as_str(), "email/work");
        assert!(p.join("a/b").is_err());
        assert!(p.join("..").is_err());
    }

    #[test]
    fn segments_iter() {
        let p = RelPath::new("a/b/c").unwrap();
        let segs: Vec<_> = p.segments().collect();
        assert_eq!(segs, vec!["a", "b", "c"]);
    }
}
