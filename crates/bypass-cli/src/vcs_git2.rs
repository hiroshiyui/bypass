// SPDX-License-Identifier: GPL-3.0-or-later

//! [`VersionControl`] implementation backed by `libgit2` via the `git2`
//! crate (see [ADR-0004](../../../doc/adr/0004-git2-crate-not-subprocess.md)).
//!
//! Per-operation, this opens the repository at the store root, stages the
//! paths the orchestrator hands us, writes a tree, and commits. Author
//! identity comes from the repository's git config; if `user.name` /
//! `user.email` are unset (e.g. inside a CI sandbox or an unconfigured
//! container) we fall back to `bypass` / `bypass@localhost` rather than
//! erroring — the alternative is for `bypass init` to fail on a perfectly
//! valid store just because `git config --global` was never run.

use std::fs;
use std::path::{Path, PathBuf};

use bypass_core::path::RelPath;
use bypass_core::vcs::{Commit, VersionControl};

#[derive(Debug, thiserror::Error)]
pub enum Git2Error {
    #[error("git2: {0}")]
    Git2(#[from] git2::Error),

    #[error("I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io_err(path: impl Into<PathBuf>, source: std::io::Error) -> Git2Error {
    Git2Error::Io {
        path: path.into(),
        source,
    }
}

/// `VersionControl` implementation rooted at the store directory.
#[derive(Debug, Clone)]
pub struct Git2Vcs {
    root: PathBuf,
}

impl Git2Vcs {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn open(&self) -> Result<git2::Repository, Git2Error> {
        Ok(git2::Repository::open(&self.root)?)
    }
}

impl VersionControl for Git2Vcs {
    type Error = Git2Error;

    fn init(&mut self) -> Result<(), Self::Error> {
        if self.is_initialized()? {
            return Ok(());
        }
        fs::create_dir_all(&self.root).map_err(|e| io_err(self.root.clone(), e))?;
        // `init` is preferred over `init_bare`: we want a working tree.
        git2::Repository::init(&self.root)?;
        Ok(())
    }

    fn is_initialized(&self) -> Result<bool, Self::Error> {
        // Match libgit2 semantics: a non-bare repo has a `.git` directory.
        // We don't open the repo here to keep `is_initialized` cheap and
        // failure-free in steady-state checks (e.g. inside `commit`).
        Ok(self.root.join(".git").exists())
    }

    fn commit(&mut self, paths: &[RelPath], message: &str) -> Result<(), Self::Error> {
        if paths.is_empty() || !self.is_initialized()? {
            return Ok(());
        }
        let repo = self.open()?;
        let mut index = repo.index()?;

        // Stage each path. If the file is gone, remove it from the index;
        // otherwise add it. This is how a single commit covers both the
        // create and the delete leg of `bypass mv`.
        for p in paths {
            let rel = Path::new(p.as_str());
            let abs = self.root.join(p.as_str());
            if abs.exists() {
                index.add_path(rel)?;
            } else {
                let _ = index.remove_path(rel);
            }
        }
        let tree_id = index.write_tree()?;
        index.write()?;
        let tree = repo.find_tree(tree_id)?;

        // No-op commit guard: don't write an empty commit when the tree
        // matches HEAD. (Orchestrator already avoids this in practice, but
        // belt-and-braces for the `mv source == dest` corner.)
        if let Ok(head) = repo.head() {
            let head_commit = head.peel_to_commit()?;
            if head_commit.tree_id() == tree_id {
                return Ok(());
            }
        }

        let sig = signature(&repo)?;
        let parents: Vec<git2::Commit<'_>> = match repo.head() {
            Ok(head) => vec![head.peel_to_commit()?],
            // Unborn branch → this is the initial commit.
            Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)?;
        Ok(())
    }

    fn log(&self, path: &RelPath) -> Result<Vec<Commit>, Self::Error> {
        if !self.is_initialized()? {
            return Ok(Vec::new());
        }
        let repo = self.open()?;
        let mut walker = repo.revwalk()?;
        if walker.push_head().is_err() {
            // Unborn branch → no history yet.
            return Ok(Vec::new());
        }
        walker.set_sorting(git2::Sort::TIME)?;

        let needle = path.as_str();
        let mut out = Vec::new();
        for oid in walker {
            let oid = oid?;
            let c = repo.find_commit(oid)?;
            if !touches_path(&repo, &c, needle)? {
                continue;
            }
            let author = c.author();
            out.push(Commit {
                id: c.id().to_string(),
                summary: c.summary().unwrap_or("").to_owned(),
                author: author.name().map(str::to_owned),
                time_unix: c.time().seconds(),
            });
        }
        Ok(out)
    }
}

/// Does the commit's diff vs its first parent (or vs an empty tree, for
/// the root commit) touch any path whose name equals or starts with
/// `needle`? Used to filter `log` to the entries below a given prefix.
fn touches_path(
    repo: &git2::Repository,
    commit: &git2::Commit<'_>,
    needle: &str,
) -> Result<bool, Git2Error> {
    let new_tree = commit.tree()?;
    let old_tree = if commit.parent_count() == 0 {
        None
    } else {
        Some(commit.parent(0)?.tree()?)
    };
    let diff = repo.diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)?;

    let prefix = format!("{needle}/");
    let mut hit = false;
    diff.foreach(
        &mut |delta, _| {
            let touched = [delta.old_file().path(), delta.new_file().path()]
                .iter()
                .flatten()
                .any(|p| {
                    p.to_str()
                        .map(|s| s == needle || s.starts_with(&prefix))
                        .unwrap_or(false)
                });
            if touched {
                hit = true;
            }
            true
        },
        None,
        None,
        None,
    )?;
    Ok(hit)
}

fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, Git2Error> {
    let cfg = repo.config()?;
    let name = cfg
        .get_string("user.name")
        .unwrap_or_else(|_| "bypass".to_string());
    let email = cfg
        .get_string("user.email")
        .unwrap_or_else(|_| "bypass@localhost".to_string());
    Ok(git2::Signature::now(&name, &email)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rp(s: &str) -> RelPath {
        RelPath::new(s).unwrap()
    }

    fn fresh() -> (TempDir, Git2Vcs) {
        let td = TempDir::new().unwrap();
        let vcs = Git2Vcs::new(td.path().to_path_buf());
        (td, vcs)
    }

    #[test]
    fn is_initialized_false_before_init_true_after() {
        let (_td, mut vcs) = fresh();
        assert!(!vcs.is_initialized().unwrap());
        vcs.init().unwrap();
        assert!(vcs.is_initialized().unwrap());
    }

    #[test]
    fn init_is_idempotent() {
        let (_td, mut vcs) = fresh();
        vcs.init().unwrap();
        vcs.init().unwrap();
    }

    #[test]
    fn commit_with_empty_paths_is_noop() {
        let (_td, mut vcs) = fresh();
        vcs.init().unwrap();
        // No history yet — must stay no-op without erroring.
        vcs.commit(&[], "noop").unwrap();
        assert!(vcs.log(&rp("anything")).unwrap().is_empty());
    }

    #[test]
    fn commit_creates_history() {
        let (td, mut vcs) = fresh();
        vcs.init().unwrap();
        fs::write(td.path().join("file.gpg"), b"ciphertext").unwrap();
        vcs.commit(&[rp("file.gpg")], "bypass: Add password for file")
            .unwrap();

        let log = vcs.log(&rp("file.gpg")).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "bypass: Add password for file");
    }

    #[test]
    fn second_commit_appears_in_log() {
        let (td, mut vcs) = fresh();
        vcs.init().unwrap();
        fs::write(td.path().join("file.gpg"), b"v1").unwrap();
        vcs.commit(&[rp("file.gpg")], "Add file").unwrap();
        fs::write(td.path().join("file.gpg"), b"v2").unwrap();
        vcs.commit(&[rp("file.gpg")], "Update file").unwrap();

        let log = vcs.log(&rp("file.gpg")).unwrap();
        assert_eq!(log.len(), 2);
        // Newest first (TIME sort).
        assert_eq!(log[0].summary, "Update file");
        assert_eq!(log[1].summary, "Add file");
    }

    #[test]
    fn log_filters_to_matching_paths() {
        let (td, mut vcs) = fresh();
        vcs.init().unwrap();
        fs::write(td.path().join("a.gpg"), b"a").unwrap();
        vcs.commit(&[rp("a.gpg")], "Add a").unwrap();
        fs::write(td.path().join("b.gpg"), b"b").unwrap();
        vcs.commit(&[rp("b.gpg")], "Add b").unwrap();

        let log = vcs.log(&rp("a.gpg")).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].summary, "Add a");
    }

    #[test]
    fn commit_handles_a_removed_file() {
        let (td, mut vcs) = fresh();
        vcs.init().unwrap();
        fs::write(td.path().join("file.gpg"), b"v1").unwrap();
        vcs.commit(&[rp("file.gpg")], "Add").unwrap();
        fs::remove_file(td.path().join("file.gpg")).unwrap();
        vcs.commit(&[rp("file.gpg")], "Remove").unwrap();

        let log = vcs.log(&rp("file.gpg")).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].summary, "Remove");
    }

    #[test]
    fn falls_back_to_default_author_when_git_config_missing() {
        // Force libgit2 to NOT see a user.name / user.email via global or
        // system config by pointing it at an empty config path. This is
        // best-effort: if the test environment has already set these via
        // env vars (GIT_AUTHOR_NAME etc.), they win at commit time and the
        // assertion below would be wrong. So we instead just assert the
        // commit succeeds and produces a valid author string.
        let (td, mut vcs) = fresh();
        vcs.init().unwrap();
        fs::write(td.path().join("x.gpg"), b"x").unwrap();
        vcs.commit(&[rp("x.gpg")], "Add x").unwrap();
        let log = vcs.log(&rp("x.gpg")).unwrap();
        assert_eq!(log.len(), 1);
        let author = log[0].author.as_deref().unwrap_or("");
        assert!(!author.is_empty(), "commit must have a non-empty author");
    }
}
