// SPDX-License-Identifier: GPL-3.0-or-later

//! Git-pack exchange over [`Transport`].
//!
//! See [ADR-0011](../../../../doc/adr/0011-sync-semantics-hybrid.md). The
//! protocol is a single request type: [`WireBody::WantPackFrom`]. The
//! responder walks from its HEAD, hides everything reachable from
//! `peer_head_seen`, packs the remainder, and returns the bytes. The
//! initiator unpacks them into the local odb, runs a leak-audit on the
//! newly-introduced commits, then either fast-forwards or rebases.
//!
//! This module is the building block: both `bypass sync` (one-shot)
//! and the `bypass sync daemon` (5.2.c) call into it. Lifetime of the
//! transport, peer discovery, and `peers.toml` iteration are the
//! caller's concern.
//!
//! DoS defences ([ADR-0016](../../../../doc/adr/0016-sync-dos-defences.md)):
//! the 50 MB pack-size cap is enforced symmetrically here (see
//! [`MAX_PACK_BYTES`]); the per-peer rate limit lives in
//! [`super::ratelimit`] and is woven in by the caller (one-shot
//! `bypass sync` or the daemon in 5.2.c).
//!
//! Multiaddr storage and mDNS-driven discovery arrive with 5.2.c.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use libp2p_identity::PeerId;

use crate::audit::{self, LeakIssue};

use super::transport::Transport;
use super::wire::{self, WireBody};

/// Maximum pack size we accept on the wire per
/// [ADR-0016](../../../../doc/adr/0016-sync-dos-defences.md). 50 MB is
/// comfortable for a password store (entries are at most a few KB and
/// history compresses well); larger stores bootstrap via a git remote.
pub const MAX_PACK_BYTES: usize = 50 * 1024 * 1024;

/// Outcome of a single peer sync.
#[derive(Debug)]
pub struct SyncReport {
    /// What we did, for human-readable logging.
    pub action: SyncAction,
    /// The peer's HEAD as of this exchange (`None` if the peer has no
    /// commits yet).
    pub peer_head: Option<String>,
}

#[derive(Debug)]
pub enum SyncAction {
    /// Peer had nothing new for us.
    UpToDate,
    /// Peer's history is a strict descendant of ours; we
    /// fast-forwarded HEAD.
    FastForwarded { from: String, to: String },
    /// Peer is behind us. They'll catch up when they pull from us
    /// next.
    PeerBehind,
    /// Both sides diverged; we rebased our work onto the peer per the
    /// peer-id tie-breaker in
    /// [ADR-0014](../../../../doc/adr/0014-sync-metadata-and-ordering.md).
    Rebased { onto: String },
    /// Both sides diverged; the tie-breaker says the *peer* rebases
    /// onto us, so we did nothing this round.
    AwaitingPeerRebase,
    /// Sync was refused on receive — leak-audit found problem files in
    /// the incoming history.
    RejectedLeak { issues: Vec<LeakIssue> },
}

/// Initiator side: ask `peer` for any commits we don't have, ingest
/// the returned pack, and reconcile HEAD.
pub async fn sync_with_peer<T: Transport>(
    transport: &T,
    peer: &T::PeerId,
    local_peer_id: &PeerId,
    remote_peer_id: &PeerId,
    root: &Path,
) -> Result<SyncReport> {
    let local_head = head_sha(root)?;
    // 5.2.b.ii: no per-peer "last seen" memory yet — that arrives with
    // the daemon's state file in 5.2.c. For now we always tell the
    // peer "I've never seen you", which means they always pack from
    // their HEAD. Wasteful on subsequent syncs but correct.
    let peer_head_seen: Option<String> = None;

    let req = wire::want_pack_from(local_head.clone(), peer_head_seen);
    let reply_bytes = transport
        .request(peer, wire::encode(&req))
        .await
        .map_err(|e| anyhow::anyhow!("transport: {e}"))?;
    let reply = wire::decode(&reply_bytes).context("decode peer reply")?;
    let (peer_head, pack_bytes) = match reply.body {
        WireBody::Pack { peer_head, bytes } => (peer_head, bytes),
        WireBody::Err { reason } => bail!("peer refused: {reason}"),
        other => bail!("unexpected peer reply: {other:?}"),
    };

    if pack_bytes.len() > MAX_PACK_BYTES {
        bail!(
            "peer pack of {} bytes exceeds {MAX_PACK_BYTES}-byte cap",
            pack_bytes.len()
        );
    }

    let Some(peer_head) = peer_head else {
        // Peer has no commits at all — nothing to do.
        return Ok(SyncReport {
            action: SyncAction::UpToDate,
            peer_head: None,
        });
    };

    if pack_bytes.is_empty() && local_head.as_deref() == Some(peer_head.as_str()) {
        return Ok(SyncReport {
            action: SyncAction::UpToDate,
            peer_head: Some(peer_head),
        });
    }

    if !pack_bytes.is_empty() {
        ingest_pack(root, &pack_bytes).context("ingest peer pack")?;
    }

    // Leak-audit the newly-introduced commits. ADR-0011 calls for a
    // symmetric leak check on receive.
    let issues = audit_incoming(root, local_head.as_deref(), &peer_head)?;
    if !issues.is_empty() {
        return Ok(SyncReport {
            action: SyncAction::RejectedLeak { issues },
            peer_head: Some(peer_head),
        });
    }

    let action = reconcile(
        root,
        local_head.as_deref(),
        &peer_head,
        local_peer_id,
        remote_peer_id,
    )?;
    Ok(SyncReport {
        action,
        peer_head: Some(peer_head),
    })
}

/// Responder side: handle one inbound `WantPackFrom` request. Returns
/// the bytes to ship back over the same transport reply slot.
pub fn serve_want_pack_from(
    root: &Path,
    local_head: Option<&str>,
    peer_head_seen: Option<&str>,
) -> Vec<u8> {
    serve_want_pack_from_capped(root, local_head, peer_head_seen, MAX_PACK_BYTES)
}

/// Same as [`serve_want_pack_from`] but with a caller-supplied cap.
/// Exists so tests can exercise the refusal path without synthesising
/// a 50 MB pack.
fn serve_want_pack_from_capped(
    root: &Path,
    local_head: Option<&str>,
    peer_head_seen: Option<&str>,
    cap: usize,
) -> Vec<u8> {
    match build_pack(root, local_head, peer_head_seen) {
        Ok(bytes) if bytes.len() > cap => wire::encode(&wire::err(format!(
            "pack of {} bytes exceeds {cap}-byte cap (ADR-0016); \
             bootstrap via a git remote first, then resume peer sync",
            bytes.len()
        ))),
        Ok(bytes) => wire::encode(&wire::pack(local_head.map(str::to_owned), bytes)),
        Err(e) => wire::encode(&wire::err(format!("build pack: {e}"))),
    }
}

/// Top-level inbound dispatcher: decode the frame, look at the body,
/// hand off to the right serve function. Used by daemon code in 5.2.c
/// and by the loopback integration test.
pub fn serve(root: &Path, bytes: &[u8]) -> Vec<u8> {
    let Ok(msg) = wire::decode(bytes) else {
        return wire::encode(&wire::err("bad wire frame"));
    };
    match msg.body {
        WireBody::WantPackFrom {
            local_head: _initiator_head,
            peer_head_seen,
        } => {
            let our_head = match head_sha(root) {
                Ok(h) => h,
                Err(e) => return wire::encode(&wire::err(format!("read HEAD: {e}"))),
            };
            serve_want_pack_from(root, our_head.as_deref(), peer_head_seen.as_deref())
        }
        WireBody::Pack { .. } | WireBody::Err { .. } => {
            wire::encode(&wire::err("only WantPackFrom is a valid inbound request"))
        }
    }
}

// ----- pack building / ingestion --------------------------------------

/// Build a git pack of everything reachable from `local_head` minus
/// everything reachable from `peer_head_seen`. Returns the raw pack
/// bytes — empty when there is nothing to send.
pub fn build_pack(
    root: &Path,
    local_head: Option<&str>,
    peer_head_seen: Option<&str>,
) -> Result<Vec<u8>> {
    let Some(head) = local_head else {
        // No HEAD yet — nothing to pack.
        return Ok(Vec::new());
    };
    let repo = git2::Repository::open(root).context("open repo for packing")?;
    let head_oid = git2::Oid::from_str(head).context("parse local HEAD oid")?;

    let mut walk = repo.revwalk().context("create revwalk")?;
    walk.push(head_oid).context("push HEAD onto revwalk")?;
    if let Some(seen) = peer_head_seen {
        // Hide everything reachable from the SHA the peer claims to
        // already have. `hide` errors if the oid isn't in the local
        // odb — in that case we ignore (peer's claim is stale) and
        // pack everything.
        if let Ok(oid) = git2::Oid::from_str(seen)
            && repo.find_commit(oid).is_ok()
        {
            walk.hide(oid).ok();
        }
    }
    // No new commits? Empty pack.
    let mut peekable = walk.peekable();
    if peekable.peek().is_none() {
        return Ok(Vec::new());
    }
    // Rebuild the walk fresh — peekable consumed `next` slot, and
    // PackBuilder::insert_walk takes the iterator state.
    let mut walk = repo.revwalk().context("recreate revwalk")?;
    walk.push(head_oid).context("re-push HEAD")?;
    if let Some(seen) = peer_head_seen
        && let Ok(oid) = git2::Oid::from_str(seen)
        && repo.find_commit(oid).is_ok()
    {
        walk.hide(oid).ok();
    }

    let mut builder = repo.packbuilder().context("create packbuilder")?;
    builder.insert_walk(&mut walk).context("pack revwalk")?;
    let mut buf = git2::Buf::new();
    builder.write_buf(&mut buf).context("serialise pack")?;
    Ok((*buf).to_vec())
}

/// Write the incoming pack bytes into the repo's odb. Uses subprocess
/// `git index-pack --stdin` so the pack ends up as a proper
/// `pack-*.pack`/`pack-*.idx` pair under `.git/objects/pack/`.
pub fn ingest_pack(root: &Path, pack_bytes: &[u8]) -> Result<()> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["index-pack", "--stdin", "--fix-thin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn `git index-pack --stdin`")?;
    {
        let mut stdin = child
            .stdin
            .take()
            .context("capture stdin of `git index-pack`")?;
        stdin
            .write_all(pack_bytes)
            .context("write pack bytes to git index-pack")?;
    }
    let out = child
        .wait_with_output()
        .context("wait for `git index-pack`")?;
    if !out.status.success() {
        bail!(
            "`git index-pack --stdin` failed (exit {}): {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

// ----- reconciliation --------------------------------------------------

fn reconcile(
    root: &Path,
    local_head: Option<&str>,
    peer_head: &str,
    local_peer_id: &PeerId,
    remote_peer_id: &PeerId,
) -> Result<SyncAction> {
    let repo = git2::Repository::open(root).context("open repo for reconcile")?;
    let peer_oid = git2::Oid::from_str(peer_head).context("parse peer HEAD oid")?;
    let local_oid = match local_head {
        Some(h) => git2::Oid::from_str(h).context("parse local HEAD oid")?,
        None => {
            // No local HEAD yet — adopt the peer's wholesale.
            fast_forward(root, peer_head)?;
            return Ok(SyncAction::FastForwarded {
                from: String::new(),
                to: peer_head.to_owned(),
            });
        }
    };

    if local_oid == peer_oid {
        return Ok(SyncAction::UpToDate);
    }

    let local_is_ancestor_of_peer = repo
        .graph_descendant_of(peer_oid, local_oid)
        .context("graph_descendant_of(peer, local)")?;
    if local_is_ancestor_of_peer {
        // Peer is strictly ahead — fast-forward.
        fast_forward(root, peer_head)?;
        return Ok(SyncAction::FastForwarded {
            from: local_oid.to_string(),
            to: peer_oid.to_string(),
        });
    }

    let peer_is_ancestor_of_local = repo
        .graph_descendant_of(local_oid, peer_oid)
        .context("graph_descendant_of(local, peer)")?;
    if peer_is_ancestor_of_local {
        return Ok(SyncAction::PeerBehind);
    }

    // Diverged. Tie-break by peer-id (ADR-0014): lower peer-id rebases
    // onto higher. Base58-encoded peer ids compare lexically.
    let local_id = local_peer_id.to_base58();
    let remote_id = remote_peer_id.to_base58();
    if local_id < remote_id {
        rebase_onto(root, peer_head)?;
        Ok(SyncAction::Rebased {
            onto: peer_head.to_owned(),
        })
    } else {
        Ok(SyncAction::AwaitingPeerRebase)
    }
}

fn fast_forward(root: &Path, target_sha: &str) -> Result<()> {
    // `git update-ref HEAD <sha>` updates HEAD without touching the
    // worktree; then `git checkout-index -af` materialises the new
    // tree. Or simply `git reset --hard <sha>` does both. Use the
    // latter — we're in a pass-store, the worktree is sacrosanct only
    // up to the audit we just ran.
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["reset", "--hard", target_sha])
        .output()
        .context("spawn `git reset --hard`")?;
    if !out.status.success() {
        bail!(
            "`git reset --hard {target_sha}` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

fn rebase_onto(root: &Path, peer_head: &str) -> Result<()> {
    // `git rebase --onto <peer_head> <merge_base> HEAD` would do the
    // surgical move, but `git rebase <peer_head>` is equivalent for
    // the common case and lets git compute the merge-base itself. The
    // custom merge driver registered via .gitattributes resolves any
    // .gpg blob conflicts by taking the incoming side.
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rebase", peer_head])
        .output()
        .context("spawn `git rebase`")?;
    if !out.status.success() {
        // Abort the half-finished rebase so the user isn't left in a
        // tangled state.
        let _ = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rebase", "--abort"])
            .status();
        bail!(
            "`git rebase {peer_head}` failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

// ----- leak audit on receive ------------------------------------------

fn audit_incoming(
    root: &Path,
    local_head: Option<&str>,
    peer_head: &str,
) -> Result<Vec<LeakIssue>> {
    // Enumerate every blob path touched by commits reachable from
    // peer_head but not from local_head.
    let range = match local_head {
        Some(h) => format!("{h}..{peer_head}"),
        None => peer_head.to_owned(),
    };
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--name-only", "--pretty=format:", &range])
        .output()
        .context("spawn `git log --name-only` for incoming audit")?;
    if !out.status.success() {
        // Treat "unknown revision" as "no new commits to audit"; the
        // pack was empty or the SHA didn't land.
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    paths.sort();
    paths.dedup();

    // Read each blob from the peer's tree via `git show <sha>:<path>`.
    // We don't want to materialise the peer's commits into the
    // worktree before the audit clears.
    let mut pairs: Vec<(PathBuf, Vec<u8>)> = Vec::with_capacity(paths.len());
    for p in paths {
        let show = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["show", &format!("{peer_head}:{}", p.display())])
            .output()
            .with_context(|| format!("spawn `git show {peer_head}:{}`", p.display()))?;
        if !show.status.success() {
            // File deleted in that commit — nothing to audit.
            continue;
        }
        pairs.push((p, show.stdout));
    }
    Ok(audit::check_files(pairs))
}

// ----- helpers ---------------------------------------------------------

fn head_sha(root: &Path) -> Result<Option<String>> {
    let repo = match git2::Repository::open(root) {
        Ok(r) => r,
        Err(e) => return Err(anyhow::Error::from(e).context("open repo for HEAD lookup")),
    };
    match repo.head() {
        Ok(reference) => match reference.peel_to_commit() {
            Ok(c) => Ok(Some(c.id().to_string())),
            Err(e) => Err(anyhow::Error::from(e).context("peel HEAD to commit")),
        },
        Err(e) if e.code() == git2::ErrorCode::UnbornBranch => Ok(None),
        Err(e) => Err(anyhow::Error::from(e).context("read HEAD")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn init_repo(name: &str, email: &str) -> TempDir {
        let td = TempDir::new().unwrap();
        run(td.path(), &["init", "-q"]);
        run(td.path(), &["config", "user.name", name]);
        run(td.path(), &["config", "user.email", email]);
        // Match the canonical .gitattributes from bypass-core.
        fs::write(
            td.path().join(".gitattributes"),
            "*.gpg binary merge=bypass-take-theirs\n",
        )
        .unwrap();
        run(td.path(), &["add", ".gitattributes"]);
        run(td.path(), &["commit", "-q", "-m", "init"]);
        td
    }

    fn run(root: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .unwrap();
        if !out.status.success() {
            panic!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    fn head(root: &Path) -> String {
        head_sha(root).unwrap().unwrap()
    }

    fn commit_entry(root: &Path, rel: &str, body: &[u8]) {
        // Write something that looks like an OpenPGP packet so the
        // leak-audit doesn't flag it: 0xC1 is a new-format packet tag.
        let mut bytes = vec![0xC1, 0x05, 0x00, 0x00, 0x00];
        bytes.extend_from_slice(body);
        fs::write(root.join(rel), bytes).unwrap();
        run(root, &["add", rel]);
        run(root, &["commit", "-q", "-m", &format!("add {rel}")]);
    }

    #[test]
    fn build_pack_returns_empty_when_peer_already_has_everything() {
        let td = init_repo("a", "a@a");
        commit_entry(td.path(), "x.gpg", b"x");
        let h = head(td.path());
        let bytes = build_pack(td.path(), Some(&h), Some(&h)).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn build_pack_returns_empty_for_unborn_head() {
        let td = TempDir::new().unwrap();
        run(td.path(), &["init", "-q"]);
        let bytes = build_pack(td.path(), None, None).unwrap();
        assert!(bytes.is_empty());
    }

    #[test]
    fn build_then_ingest_round_trips_a_commit_to_a_fresh_clone() {
        let src = init_repo("src", "src@s");
        commit_entry(src.path(), "alpha.gpg", b"alpha");
        let src_head = head(src.path());

        // Build a pack of everything (peer has nothing).
        let pack = build_pack(src.path(), Some(&src_head), None).unwrap();
        assert!(!pack.is_empty());

        // Fresh repo, ingest, verify the commit + blob exist.
        let dst = TempDir::new().unwrap();
        run(dst.path(), &["init", "-q"]);
        ingest_pack(dst.path(), &pack).unwrap();
        // git cat-file -e <sha> exits 0 iff the object exists.
        let exists = Command::new("git")
            .arg("-C")
            .arg(dst.path())
            .args(["cat-file", "-e", &src_head])
            .status()
            .unwrap();
        assert!(exists.success(), "ingested commit should exist in dst odb");
    }

    #[test]
    fn build_pack_excludes_commits_peer_already_has() {
        let src = init_repo("src", "src@s");
        commit_entry(src.path(), "a.gpg", b"a");
        let head_a = head(src.path());
        commit_entry(src.path(), "b.gpg", b"b");
        let head_b = head(src.path());

        // Peer claims to have head_a; pack should only contain the new
        // b.gpg commit.
        let pack_full = build_pack(src.path(), Some(&head_b), None).unwrap();
        let pack_delta = build_pack(src.path(), Some(&head_b), Some(&head_a)).unwrap();
        assert!(!pack_delta.is_empty());
        assert!(
            pack_delta.len() < pack_full.len(),
            "delta pack should be smaller than full pack"
        );
    }

    #[test]
    fn serve_round_trips_through_wire_encoding() {
        let src = init_repo("src", "src@s");
        commit_entry(src.path(), "x.gpg", b"x");
        let src_head = head(src.path());

        let req = wire::encode(&wire::want_pack_from(None, None));
        let reply_bytes = serve(src.path(), &req);
        let reply = wire::decode(&reply_bytes).unwrap();
        match reply.body {
            WireBody::Pack { peer_head, bytes } => {
                assert_eq!(peer_head.as_deref(), Some(src_head.as_str()));
                assert!(!bytes.is_empty());
            }
            other => panic!("expected Pack, got {other:?}"),
        }
    }

    #[test]
    fn serve_refuses_when_pack_exceeds_cap() {
        let src = init_repo("src", "src@s");
        commit_entry(src.path(), "x.gpg", b"x");
        let src_head = head(src.path());
        // Use a tiny cap (10 bytes) so even a minimal real pack trips it.
        let reply_bytes = serve_want_pack_from_capped(src.path(), Some(&src_head), None, 10);
        let reply = wire::decode(&reply_bytes).unwrap();
        match reply.body {
            WireBody::Err { reason } => {
                assert!(reason.contains("cap"), "{reason}");
                assert!(reason.contains("ADR-0016"), "{reason}");
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[test]
    fn serve_rejects_unexpected_inbound_frames() {
        let td = init_repo("x", "x@x");
        let bytes = wire::encode(&wire::pack(None, vec![]));
        let reply = wire::decode(&serve(td.path(), &bytes)).unwrap();
        match reply.body {
            WireBody::Err { reason } => assert!(reason.contains("WantPackFrom"), "{reason}"),
            other => panic!("expected Err, got {other:?}"),
        }
    }
}
