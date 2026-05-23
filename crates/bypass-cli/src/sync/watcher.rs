// SPDX-License-Identifier: GPL-3.0-or-later

//! Filesystem watcher for the password store.
//!
//! The 5.2.c daemon ([`sync::daemon`](super::daemon)) wants to know
//! when the store has changed so it can push the new history to
//! paired peers. We wrap [`notify::RecommendedWatcher`] (inotify on
//! Linux) with two pieces of policy:
//!
//! 1. **Filter**: events under `<root>/.git/` are dropped. Without
//!    this, our own pack-ingest writes (`git index-pack` into
//!    `.git/objects/`) would trigger a sync round of our own,
//!    forming a feedback loop with the peer that just pushed to us.
//!    Events on `<root>/.gitattributes` and `<root>/.gpg-id` *are*
//!    relevant — the daemon should re-sync when the user rotates
//!    recipients.
//! 2. **Debounce**: bursty changes (an editor saving a .gpg blob
//!    plus the auto-commit's `.git/index` update plus the new pack
//!    object) collapse into a single tick. A 500 ms window keeps
//!    the daemon from launching three sync rounds for what is
//!    semantically one user action.
//!
//! The output is an `mpsc::Receiver<()>` that the daemon
//! `tokio::select!`s on; each `()` means "something under the
//! store changed, consider syncing."

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use notify::{EventKind, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// Debounce window. A burst of events within this period collapse
/// into one tick downstream.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Path component we always ignore. Writes under here are usually
/// ours (pack ingest, auto-commit). The user's `.gitattributes` /
/// `.gpg-id` live at `<root>/.gitattributes` directly, not under
/// `.git/`, so they are not filtered.
const GIT_INTERNAL: &str = ".git";

/// Spawn a watcher rooted at `root`. Returns the receiver end of an
/// `mpsc` channel that emits one `()` per debounce window in which
/// at least one relevant event occurred. The returned guard owns
/// the underlying [`notify::Watcher`]; dropping it stops the
/// watcher and closes the channel.
pub fn watch(root: &Path) -> Result<WatcherHandle> {
    let root = root.to_path_buf();
    let (raw_tx, mut raw_rx) = mpsc::unbounded_channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        // Best-effort: if the daemon dropped the receiver, the
        // mpsc::send fails silently and the watcher just stops
        // shipping events. That happens at shutdown.
        let _ = raw_tx.send(res);
    })
    .context("create filesystem watcher")?;

    watcher
        .watch(&root, RecursiveMode::Recursive)
        .with_context(|| format!("watch {}", root.display()))?;

    // The downstream debounced channel. Bounded at 1: we never
    // need to queue more than "yes, something happened"; the
    // daemon will drain it.
    let (debounced_tx, debounced_rx) = mpsc::channel::<()>(1);
    let pump = tokio::spawn(async move {
        let mut pending = false;
        let mut deadline: Option<tokio::time::Instant> = None;
        loop {
            let timer = match deadline {
                Some(d) => tokio::time::sleep_until(d),
                // No pending tick — sleep "forever"; the recv() arm
                // wakes us when something arrives.
                None => tokio::time::sleep(Duration::from_secs(60 * 60 * 24)),
            };
            tokio::pin!(timer);
            tokio::select! {
                ev = raw_rx.recv() => {
                    let Some(ev) = ev else { break; };
                    if let Ok(event) = ev
                        && event_is_relevant(&event, &root)
                    {
                        pending = true;
                        deadline = Some(tokio::time::Instant::now() + DEBOUNCE);
                    }
                }
                _ = &mut timer, if deadline.is_some() => {
                    if pending {
                        // Try-send: if the daemon hasn't drained yet,
                        // a second tick would be redundant.
                        let _ = debounced_tx.try_send(());
                        pending = false;
                    }
                    deadline = None;
                }
            }
        }
    });

    Ok(WatcherHandle {
        watcher,
        rx: debounced_rx,
        _pump: pump,
    })
}

/// Owns the watcher's lifetime. Drop it to stop. The receiver end is
/// borrowed mutably via [`Self::rx`].
pub struct WatcherHandle {
    // Kept alive so the watcher thread keeps running.
    #[allow(dead_code)]
    watcher: notify::RecommendedWatcher,
    rx: mpsc::Receiver<()>,
    // Aborted on drop so we don't leak the background pump.
    _pump: tokio::task::JoinHandle<()>,
}

impl WatcherHandle {
    /// Mutable borrow of the receiver. The daemon does
    /// `handle.rx().recv()` inside its `tokio::select!`.
    pub fn rx(&mut self) -> &mut mpsc::Receiver<()> {
        &mut self.rx
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        // `notify::RecommendedWatcher` is dropped naturally; abort
        // the pump task to release its `raw_rx` handle.
        self._pump.abort();
    }
}

fn event_is_relevant(event: &notify::Event, root: &Path) -> bool {
    // Filter event kinds: chmod-only, access-only, "other" — skip.
    // We care about creates/modifies/removes.
    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
        _ => return false,
    }
    // Drop anything under `.git/`.
    event.paths.iter().any(|p| !path_is_inside_git_dir(p, root))
}

fn path_is_inside_git_dir(p: &Path, root: &Path) -> bool {
    // We compare components relative to `root`. If the relative path
    // starts with `.git/` (or *is* `.git`), it's the internal dir.
    let Ok(rel) = p.strip_prefix(root) else {
        // Outside the watched tree — count as "inside .git" so we
        // don't trigger on something unexpected.
        return true;
    };
    rel.components()
        .next()
        .is_some_and(|c| c.as_os_str().to_str().is_some_and(|s| s == GIT_INTERNAL))
}

#[allow(dead_code)]
fn relative_to(root: &Path, p: &Path) -> Option<PathBuf> {
    p.strip_prefix(root).ok().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn make_event(kind: EventKind, paths: &[&Path]) -> notify::Event {
        notify::Event {
            kind,
            paths: paths.iter().map(|p| p.to_path_buf()).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn event_under_root_but_outside_git_is_relevant() {
        let root = Path::new("/store");
        let ev = make_event(
            EventKind::Create(notify::event::CreateKind::File),
            &[Path::new("/store/email/work.gpg")],
        );
        assert!(event_is_relevant(&ev, root));
    }

    #[test]
    fn event_under_dot_git_is_filtered() {
        let root = Path::new("/store");
        let ev = make_event(
            EventKind::Modify(notify::event::ModifyKind::Any),
            &[Path::new("/store/.git/index")],
        );
        assert!(!event_is_relevant(&ev, root));
    }

    #[test]
    fn event_on_gitattributes_at_root_is_relevant() {
        // `.gitattributes` lives at the store root, NOT under .git/.
        let root = Path::new("/store");
        let ev = make_event(
            EventKind::Modify(notify::event::ModifyKind::Any),
            &[Path::new("/store/.gitattributes")],
        );
        assert!(event_is_relevant(&ev, root));
    }

    #[test]
    fn event_kind_access_is_ignored() {
        let root = Path::new("/store");
        let ev = make_event(
            EventKind::Access(notify::event::AccessKind::Read),
            &[Path::new("/store/email/work.gpg")],
        );
        assert!(!event_is_relevant(&ev, root));
    }

    #[tokio::test]
    async fn writes_under_root_trigger_a_debounced_tick() {
        let td = TempDir::new().unwrap();
        let mut handle = watch(td.path()).unwrap();

        fs::write(td.path().join("entry.gpg"), b"ciphertext").unwrap();
        // Debounce window plus scheduling slack for CI-runner noise.
        let tick = tokio::time::timeout(Duration::from_secs(2), handle.rx().recv()).await;
        assert!(tick.is_ok() && tick.unwrap().is_some(), "expected a tick");
    }

    #[tokio::test]
    async fn writes_under_dot_git_do_not_trigger_a_tick() {
        let td = TempDir::new().unwrap();
        let git = td.path().join(".git");
        fs::create_dir(&git).unwrap();
        let mut handle = watch(td.path()).unwrap();

        fs::write(git.join("index"), b"git internal").unwrap();
        // If a tick was going to land, it would by debounce + slack.
        let tick =
            tokio::time::timeout(DEBOUNCE + Duration::from_secs(1), handle.rx().recv()).await;
        assert!(tick.is_err(), "should have timed out; got {tick:?}");
    }
}
