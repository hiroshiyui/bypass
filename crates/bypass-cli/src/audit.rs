// SPDX-License-Identifier: GPL-3.0-or-later

//! Leak-check audit: verify that the files about to be published look
//! like OpenPGP ciphertext (or recognised store metadata), so a `bypass
//! sync` never publishes accidental plaintext.
//!
//! See [ADR-0009](../../../doc/adr/0009-leak-check-before-push.md).
//!
//! The checker itself is pure: it takes `(path, bytes)` tuples and
//! returns a `Vec<LeakIssue>`. The disk-walking convenience
//! `audit_for_push` lives at the bottom of the module and is the entry
//! point used by `bypass sync` / `bypass audit` / `bypass doctor`.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// Why a file was flagged as a leak risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeakKind {
    /// Filename matches an editor-backup pattern (`*~`, `.*.swp`,
    /// `*.orig`, `*.rej`, `*.bak`, `#*#`). These can hold partial
    /// plaintext from an interrupted edit.
    EditorBackup,
    /// File ends in `.gpg` but its head does not look like an OpenPGP
    /// packet (binary or ASCII armour).
    NotEncrypted,
    /// Filename is not one of the recognised store conventions
    /// (`*.gpg`, `.gpg-id`, `.gpg-id.sig`, `.gitignore`,
    /// `.gitattributes`, `README*`, `LICENSE*`).
    UnknownFilename,
}

impl LeakKind {
    pub fn describe(self) -> &'static str {
        match self {
            Self::EditorBackup => "editor backup",
            Self::NotEncrypted => "not encrypted",
            Self::UnknownFilename => "unknown filename",
        }
    }
}

/// A single suspected leak.
#[derive(Debug, Clone)]
pub struct LeakIssue {
    pub path: PathBuf,
    pub kind: LeakKind,
    pub detail: String,
}

/// Classify a single file. Returns `None` if the file looks fine.
pub fn check_file(path: &Path, head: &[u8]) -> Option<LeakIssue> {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");

    if is_editor_backup(name) {
        return Some(LeakIssue {
            path: path.to_owned(),
            kind: LeakKind::EditorBackup,
            detail: "editor-backup naming pattern".into(),
        });
    }

    if name.ends_with(".gpg") {
        if !looks_like_openpgp(head) {
            return Some(LeakIssue {
                path: path.to_owned(),
                kind: LeakKind::NotEncrypted,
                detail: "no OpenPGP packet or ASCII-armour header".into(),
            });
        }
        return None;
    }

    if is_allowlisted_metadata(name) {
        return None;
    }

    Some(LeakIssue {
        path: path.to_owned(),
        kind: LeakKind::UnknownFilename,
        detail: "not `*.gpg` or recognised metadata".into(),
    })
}

/// Run [`check_file`] over a collection of `(path, bytes)` pairs.
pub fn check_files<I, P>(files: I) -> Vec<LeakIssue>
where
    I: IntoIterator<Item = (P, Vec<u8>)>,
    P: AsRef<Path>,
{
    let mut out = Vec::new();
    for (path, bytes) in files {
        if let Some(issue) = check_file(path.as_ref(), &bytes) {
            out.push(issue);
        }
    }
    out
}

/// Enumerate the files that `git push` would publish from `store_root`,
/// load each from the working tree, and return any leak issues. Empty
/// vec means "all clear".
pub fn audit_for_push(store_root: &Path) -> Result<Vec<LeakIssue>> {
    let files = files_to_audit(store_root)?;
    let mut pairs: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(files.len());
    for rel in &files {
        let abs = store_root.join(rel);
        // It's OK for a file listed in the diff to be absent (a delete
        // commit). Just skip it; you can't leak what isn't on disk.
        let bytes = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(
                    anyhow::Error::from(e).context(format!("read {} for audit", abs.display()))
                );
            }
        };
        pairs.push((rel.clone(), bytes));
    }
    Ok(check_files(pairs))
}

/// Resolve the set of pathnames to audit. Tries `git diff --name-only
/// @{upstream}..HEAD` first; if no upstream is configured (initial sync)
/// or the ref is missing, falls back to `git ls-files`.
fn files_to_audit(store_root: &Path) -> Result<Vec<PathBuf>> {
    if let Some(diff) = git_diff_against_upstream(store_root)? {
        return Ok(diff);
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(store_root)
        .args(["ls-files"])
        .output()
        .context("spawn `git ls-files`")?;
    if !out.status.success() {
        anyhow::bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(parse_pathlist(&out.stdout))
}

fn git_diff_against_upstream(store_root: &Path) -> Result<Option<Vec<PathBuf>>> {
    // `rev-parse @{upstream}` succeeds only when an upstream is set
    // *and* the ref exists locally (i.e. we've fetched at least once).
    let probe = Command::new("git")
        .arg("-C")
        .arg(store_root)
        .args(["rev-parse", "--verify", "--quiet", "@{upstream}"])
        .output()
        .context("spawn `git rev-parse @{upstream}`")?;
    if !probe.status.success() {
        return Ok(None);
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(store_root)
        .args(["diff", "--name-only", "@{upstream}..HEAD"])
        .output()
        .context("spawn `git diff --name-only @{upstream}..HEAD`")?;
    if !out.status.success() {
        anyhow::bail!("git diff failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(Some(parse_pathlist(&out.stdout)))
}

fn parse_pathlist(bytes: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect()
}

// ----- predicates ------------------------------------------------------

/// OpenPGP packet header sniff per RFC 4880 §4.2:
///
/// - Old format: first byte `0b10xxxxxx` → `0x80..=0xBF`.
/// - New format: first byte `0b11xxxxxx` → `0xC0..=0xFF`.
/// - ASCII armour: `-----BEGIN PGP MESSAGE-----` prefix.
///
/// We deliberately don't parse the packet; a valid header rules out
/// plaintext, which is the entire point of this check.
fn looks_like_openpgp(head: &[u8]) -> bool {
    if let Some(&first) = head.first()
        && (0x80..=0xFF).contains(&first)
    {
        return true;
    }
    head.starts_with(b"-----BEGIN PGP MESSAGE-----")
}

fn is_allowlisted_metadata(name: &str) -> bool {
    if matches!(
        name,
        ".gpg-id" | ".gpg-id.sig" | ".gitignore" | ".gitattributes"
    ) {
        return true;
    }
    if name.starts_with("README") || name.starts_with("LICENSE") {
        return true;
    }
    false
}

fn is_editor_backup(name: &str) -> bool {
    // vim swap file: .<name>.swp or .<name>.swo
    if name.starts_with('.') && (name.ends_with(".swp") || name.ends_with(".swo")) {
        return true;
    }
    // emacs autosave: #<name>#
    if name.starts_with('#') && name.ends_with('#') && name.len() >= 2 {
        return true;
    }
    name.ends_with('~')
        || name.ends_with(".orig")
        || name.ends_with(".rej")
        || name.ends_with(".bak")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn binary_gpg_with_new_format_packet_header_is_accepted() {
        // 0xC1 = new-format header, tag 1 (PKESK).
        let head = [0xC1, 0x05, 0x00, 0x00, 0x00];
        assert!(check_file(&path("email/work.gpg"), &head).is_none());
    }

    #[test]
    fn binary_gpg_with_old_format_packet_header_is_accepted() {
        // 0x85 = old-format header, tag 1, 2-octet length.
        let head = [0x85, 0x00, 0x10];
        assert!(check_file(&path("a.gpg"), &head).is_none());
    }

    #[test]
    fn ascii_armoured_gpg_is_accepted() {
        let head = b"-----BEGIN PGP MESSAGE-----\nVersion: GnuPG\n";
        assert!(check_file(&path("a.gpg"), head).is_none());
    }

    #[test]
    fn gpg_file_with_plaintext_is_flagged_as_not_encrypted() {
        let issue = check_file(&path("a.gpg"), b"this is a password\n").unwrap();
        assert_eq!(issue.kind, LeakKind::NotEncrypted);
    }

    #[test]
    fn unknown_filename_is_flagged() {
        let issue = check_file(&path("notes.txt"), b"hello").unwrap();
        assert_eq!(issue.kind, LeakKind::UnknownFilename);
    }

    #[test]
    fn gpg_id_and_friends_are_allowed() {
        for n in [".gpg-id", ".gpg-id.sig", ".gitignore", ".gitattributes"] {
            assert!(
                check_file(&path(n), b"...").is_none(),
                "{n} should be allowed"
            );
        }
        assert!(check_file(&path("README.md"), b"# x").is_none());
        assert!(check_file(&path("LICENSE"), b"GPL").is_none());
    }

    #[test]
    fn editor_backups_are_flagged() {
        for n in [
            "work.gpg~",
            ".work.gpg.swp",
            ".work.gpg.swo",
            "work.gpg.orig",
            "work.gpg.rej",
            "work.gpg.bak",
            "#work.gpg#",
        ] {
            let issue = check_file(&path(n), b"whatever").unwrap();
            assert_eq!(
                issue.kind,
                LeakKind::EditorBackup,
                "{n} should be flagged as editor backup"
            );
        }
    }

    #[test]
    fn empty_gpg_file_is_flagged_as_not_encrypted() {
        // No bytes → no packet header → leak.
        let issue = check_file(&path("a.gpg"), b"").unwrap();
        assert_eq!(issue.kind, LeakKind::NotEncrypted);
    }

    #[test]
    fn check_files_aggregates_multiple_issues() {
        let files: Vec<(PathBuf, Vec<u8>)> = vec![
            (path("a.gpg"), vec![0xC1, 0x00]),      // ok
            (path("notes.txt"), b"hello".to_vec()), // unknown
            (path("a.gpg.bak"), b"hello".to_vec()), // backup
        ];
        let issues = check_files(files);
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|i| i.kind == LeakKind::UnknownFilename));
        assert!(issues.iter().any(|i| i.kind == LeakKind::EditorBackup));
    }
}
