// SPDX-License-Identifier: GPL-3.0-or-later

//! `bypass sync daemon` main loop.
//!
//! Holds the libp2p Swarm via [`Libp2pTransport`], the local mirror
//! of [`peers::Peers`], a per-peer rate limit
//! ([`super::ratelimit::AttemptLog`]) and a state snapshot for
//! [`sync::socket::serve_status`]. Drives one
//! [`tokio::select!`] over the four sources of work:
//!
//! 1. **Inbound RPCs** from paired peers (`WantPackFrom`) — answered
//!    via [`syncing::serve`].
//! 2. **mDNS discovery** events — for paired peers, kick off an
//!    outbound sync.
//! 3. **Filesystem watcher** ticks — sync to every paired peer we
//!    have a known multiaddr for.
//! 4. **Status socket** connections — answer with a snapshot.
//!
//! `SIGTERM` / `Ctrl-C` end the loop and let the program exit.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::libp2p_transport::{DiscoveryEvent, Libp2pTransport};
use super::peers::{PeerRecord, Peers};
use super::ratelimit::AttemptLog;
use super::socket::{PeerStatus, StatusSnapshot};
use super::syncing::{self, SyncAction};
use super::transport::Transport;
use super::watcher::WatcherHandle;
use anyhow::{Context, Result};
use libp2p::Multiaddr;
use libp2p_identity::PeerId;

/// Mutable per-peer record the daemon keeps in RAM. Persisted bits
/// (the pinned identity, the friendly name) come from
/// [`PeerRecord`]; runtime bits (last sync action, known multiaddr,
/// "currently dialing" flag) live only here.
#[derive(Debug, Clone)]
struct PeerEntry {
    name: String,
    /// Most recent multiaddr we've seen on the LAN; `None` until
    /// mDNS lights it up.
    addr: Option<Multiaddr>,
    /// Whether mDNS currently sees the peer. `true` between a
    /// `Discovered` and the matching `Expired`.
    discovered: bool,
    /// Last `SyncAction` (as its variant name) and the unix
    /// timestamp it was logged.
    last_sync: Option<(String, u64)>,
    /// True while an outbound sync to this peer is in flight; used
    /// to coalesce a discovery burst into one dial.
    in_flight: bool,
}

/// Shared daemon state. Protected by a `std::sync::Mutex` because
/// every critical section is short (an insert, a snapshot read,
/// an mtime check) — no `.await` is ever held under this lock.
#[derive(Debug)]
struct DaemonState {
    /// Keyed by peer-id (parsed from `peers.toml::PeerRecord::peer_id`).
    peers: HashMap<PeerId, PeerEntry>,
    /// `peers.toml` path so the daemon can hot-reload after a
    /// `bypass sync peer rm` mutates it.
    peers_toml: PathBuf,
    /// mtime of `peers.toml` at last load. Compared on every reload
    /// trigger so we skip parsing when nothing changed.
    peers_toml_mtime: Option<SystemTime>,
}

impl DaemonState {
    fn snapshot(&self, local_peer_id: PeerId, listening_addrs: Vec<Multiaddr>) -> StatusSnapshot {
        let mut peers: Vec<PeerStatus> = self
            .peers
            .iter()
            .map(|(pid, entry)| PeerStatus {
                name: entry.name.clone(),
                peer_id: pid.to_base58(),
                discovered: entry.discovered,
                last_sync_action: entry.last_sync.as_ref().map(|(a, _)| a.clone()),
                last_sync_unix: entry.last_sync.as_ref().map(|(_, t)| *t),
            })
            .collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        StatusSnapshot {
            local_peer_id: local_peer_id.to_base58(),
            listening_addrs: listening_addrs.iter().map(|a| a.to_string()).collect(),
            peers,
        }
    }

    /// Reload `peers.toml` if its mtime changed since last load.
    /// Used to pick up `bypass sync peer rm` without an SIGHUP.
    fn reload_peers_if_changed(&mut self) -> Result<()> {
        let meta = match std::fs::metadata(&self.peers_toml) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // File was deleted entirely — drop every pin.
                self.peers.clear();
                self.peers_toml_mtime = None;
                return Ok(());
            }
            Err(e) => return Err(e).context("stat peers.toml"),
        };
        let mtime = meta.modified().ok();
        if mtime == self.peers_toml_mtime {
            return Ok(());
        }
        let fresh = Peers::load(&self.peers_toml)?;
        self.apply_peers(&fresh);
        self.peers_toml_mtime = mtime;
        Ok(())
    }

    fn apply_peers(&mut self, peers: &Peers) {
        let mut keep: HashMap<PeerId, PeerEntry> = HashMap::new();
        for rec in peers.records() {
            let Ok(pid) = rec.peer_id.parse::<PeerId>() else {
                continue;
            };
            // Preserve runtime state for peers that survived.
            let entry = self
                .peers
                .remove(&pid)
                .map(|mut e| {
                    e.name = rec.name.clone();
                    e
                })
                .unwrap_or_else(|| PeerEntry::from_record(rec));
            keep.insert(pid, entry);
        }
        // Anything left in self.peers was removed by the user; drop.
        self.peers = keep;
    }
}

impl PeerEntry {
    fn from_record(rec: &PeerRecord) -> Self {
        Self {
            name: rec.name.clone(),
            addr: None,
            discovered: false,
            last_sync: None,
            in_flight: false,
        }
    }
}

/// Public daemon entry-point. Owns the runtime lifetime: returns
/// only on signal shutdown or a fatal listener error.
pub async fn run(
    root: PathBuf,
    transport: Libp2pTransport,
    initial_peers: Peers,
    peers_toml: PathBuf,
    mut watcher: WatcherHandle,
    status_listener: tokio::net::UnixListener,
) -> Result<()> {
    let local_peer_id = transport.local_peer_id();
    let transport = Arc::new(transport);

    let state = Arc::new(Mutex::new(DaemonState {
        peers: HashMap::new(),
        peers_toml: peers_toml.clone(),
        peers_toml_mtime: std::fs::metadata(&peers_toml)
            .ok()
            .and_then(|m| m.modified().ok()),
    }));
    state.lock().unwrap().apply_peers(&initial_peers);

    let ratelimit = Arc::new(Mutex::new(AttemptLog::<PeerId>::new()));

    // Status socket: spawn the accept loop with a snapshot closure
    // that reaches into the shared state.
    let snapshot_transport = Arc::clone(&transport);
    let snapshot_state = Arc::clone(&state);
    let snapshot_fn = move || {
        let listen = snapshot_transport.listen_addrs();
        snapshot_state
            .lock()
            .unwrap()
            .snapshot(local_peer_id, listen)
    };
    let status_task = tokio::spawn(super::socket::serve_status(status_listener, snapshot_fn));

    // Signal handling. ctrl_c() resolves on SIGINT; SIGTERM is
    // captured via tokio::signal::unix.
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;

    eprintln!(
        "bypass-sync: daemon up, peer-id {}, listening on {} addr(s)",
        local_peer_id.to_base58(),
        transport.listen_addrs().len()
    );

    loop {
        tokio::select! {
            // Inbound RPC: answer immediately. The pack-build is
            // CPU-bound but quick enough at our store sizes; if it
            // ever isn't, spawn_blocking. Held inline so the
            // per-peer rate-limit / pinning check is impossible to
            // bypass.
            inbound = transport.next_request() => {
                let Ok((peer, bytes, reply)) = inbound else {
                    eprintln!("bypass-sync: transport closed; shutting down");
                    break;
                };
                handle_inbound(
                    &root,
                    &peer,
                    bytes,
                    reply,
                    &state,
                    &ratelimit,
                );
            }
            // mDNS discovery: light up the entry's `discovered`
            // flag and stash the addr; if the peer is paired,
            // schedule an outbound sync.
            disco = transport.next_discovery() => {
                let Some(event) = disco else { break; };
                let to_dial = handle_discovery(&event, &state);
                if let Some((peer, addr)) = to_dial {
                    spawn_sync_with_peer(
                        Arc::clone(&transport),
                        &root,
                        &state,
                        &ratelimit,
                        local_peer_id,
                        peer,
                        addr,
                    );
                }
            }
            // FS watcher: try to sync with every paired peer we
            // already have a known address for.
            Some(()) = watcher.rx().recv() => {
                let candidates = collect_known_addrs(&state);
                for (peer, addr) in candidates {
                    spawn_sync_with_peer(
                        Arc::clone(&transport),
                        &root,
                        &state,
                        &ratelimit,
                        local_peer_id,
                        peer,
                        addr,
                    );
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("bypass-sync: SIGINT received, exiting");
                break;
            }
            _ = sigterm.recv() => {
                eprintln!("bypass-sync: SIGTERM received, exiting");
                break;
            }
        }
    }

    status_task.abort();
    Ok(())
}

fn handle_inbound(
    root: &Path,
    peer: &PeerId,
    bytes: Vec<u8>,
    reply: super::transport::Reply,
    state: &Arc<Mutex<DaemonState>>,
    ratelimit: &Arc<Mutex<AttemptLog<PeerId>>>,
) {
    // Pinning check: must be in peers.toml (after a hot-reload).
    {
        let mut st = state.lock().unwrap();
        if let Err(e) = st.reload_peers_if_changed() {
            eprintln!("bypass-sync: peers.toml reload failed: {e:#}");
        }
        if !st.peers.contains_key(peer) {
            let response = super::wire::encode(&super::wire::err("not a paired peer"));
            reply.send(response);
            return;
        }
    }
    // Rate-limit check: 3 / 60 s per peer (ADR-0016).
    {
        let mut rl = ratelimit.lock().unwrap();
        if rl.check_and_record(peer).is_err() {
            let response = super::wire::encode(&super::wire::err(
                "rate-limited (ADR-0016: 3 attempts / 60 s)",
            ));
            reply.send(response);
            return;
        }
    }
    let response = syncing::serve(root, &bytes);
    reply.send(response);
}

/// Update state for a discovery event. Returns a `(peer, addr)`
/// tuple iff the event names a paired peer that we should dial
/// (and we aren't already mid-dial to them).
fn handle_discovery(
    event: &DiscoveryEvent,
    state: &Arc<Mutex<DaemonState>>,
) -> Option<(PeerId, Multiaddr)> {
    let mut st = state.lock().unwrap();
    if let Err(e) = st.reload_peers_if_changed() {
        eprintln!("bypass-sync: peers.toml reload failed: {e:#}");
    }
    match event {
        DiscoveryEvent::Discovered { peer, addr } => {
            let entry = st.peers.get_mut(peer)?;
            entry.discovered = true;
            entry.addr = Some(addr.clone());
            if entry.in_flight {
                None
            } else {
                entry.in_flight = true;
                Some((*peer, addr.clone()))
            }
        }
        DiscoveryEvent::Expired { peer, .. } => {
            if let Some(entry) = st.peers.get_mut(peer) {
                entry.discovered = false;
            }
            None
        }
    }
}

fn collect_known_addrs(state: &Arc<Mutex<DaemonState>>) -> Vec<(PeerId, Multiaddr)> {
    let mut st = state.lock().unwrap();
    let mut out = Vec::new();
    for (pid, entry) in st.peers.iter_mut() {
        if let Some(addr) = &entry.addr
            && !entry.in_flight
        {
            entry.in_flight = true;
            out.push((*pid, addr.clone()));
        }
    }
    out
}

fn spawn_sync_with_peer(
    transport: Arc<Libp2pTransport>,
    root: &Path,
    state: &Arc<Mutex<DaemonState>>,
    ratelimit: &Arc<Mutex<AttemptLog<PeerId>>>,
    local_peer_id: PeerId,
    peer: PeerId,
    addr: Multiaddr,
) {
    // Outbound rate-limit: the same per-peer budget as inbound,
    // so a flaky peer doesn't trigger reconnect storms.
    {
        let mut rl = ratelimit.lock().unwrap();
        if rl.check_and_record(&peer).is_err() {
            // Quietly drop; the in-flight flag will clear below.
            let mut st = state.lock().unwrap();
            if let Some(entry) = st.peers.get_mut(&peer) {
                entry.in_flight = false;
            }
            return;
        }
    }

    let root = root.to_path_buf();
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let result = run_one_sync(&transport, &root, &local_peer_id, &peer, &addr).await;
        let mut st = state.lock().unwrap();
        if let Some(entry) = st.peers.get_mut(&peer) {
            entry.in_flight = false;
            match result {
                Ok(action_name) => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    entry.last_sync = Some((action_name, now));
                }
                Err(e) => {
                    eprintln!("bypass-sync: sync with {} failed: {e:#}", peer.to_base58());
                }
            }
        }
    });
}

async fn run_one_sync(
    transport: &Libp2pTransport,
    root: &Path,
    local_peer_id: &PeerId,
    peer: &PeerId,
    addr: &Multiaddr,
) -> Result<String> {
    transport.dial(*peer, addr.clone()).await?;
    // Give the dial → Noise → substream handshake time to settle.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let report = syncing::sync_with_peer(transport, peer, local_peer_id, peer, root).await?;
    Ok(sync_action_name(&report.action))
}

fn sync_action_name(action: &SyncAction) -> String {
    match action {
        SyncAction::UpToDate => "UpToDate",
        SyncAction::FastForwarded { .. } => "FastForwarded",
        SyncAction::PeerBehind => "PeerBehind",
        SyncAction::Rebased { .. } => "Rebased",
        SyncAction::AwaitingPeerRebase => "AwaitingPeerRebase",
        SyncAction::RejectedLeak { .. } => "RejectedLeak",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn td_with_peer(name: &str, peer_id: &str) -> (TempDir, PathBuf, Peers) {
        let td = TempDir::new().unwrap();
        let peers_path = td.path().join("peers.toml");
        let mut peers = Peers::default();
        peers.upsert(PeerRecord {
            name: name.into(),
            peer_id: peer_id.into(),
            noise_static_key: "dead".into(),
            paired_at: "2026-01-01T00:00:00Z".into(),
        });
        peers.save(&peers_path).unwrap();
        (td, peers_path, peers)
    }

    #[test]
    fn snapshot_lists_pinned_peers_alphabetically_by_name() {
        // Two real peer-ids generated from random keypairs so the
        // base58 strings round-trip through the PeerId parser.
        let kp_a = libp2p_identity::Keypair::generate_ed25519();
        let kp_b = libp2p_identity::Keypair::generate_ed25519();
        let pid_a = PeerId::from(kp_a.public()).to_base58();
        let pid_b = PeerId::from(kp_b.public()).to_base58();

        let mut peers = Peers::default();
        peers.upsert(PeerRecord {
            name: "zebra".into(),
            peer_id: pid_a.clone(),
            noise_static_key: "x".into(),
            paired_at: "2026-01-01T00:00:00Z".into(),
        });
        peers.upsert(PeerRecord {
            name: "apple".into(),
            peer_id: pid_b.clone(),
            noise_static_key: "y".into(),
            paired_at: "2026-01-01T00:00:00Z".into(),
        });
        let mut state = DaemonState {
            peers: HashMap::new(),
            peers_toml: PathBuf::from("/dev/null"),
            peers_toml_mtime: None,
        };
        state.apply_peers(&peers);
        let local = PeerId::from(kp_a.public());
        let snap = state.snapshot(local, vec![]);
        assert_eq!(snap.peers.len(), 2);
        assert_eq!(snap.peers[0].name, "apple");
        assert_eq!(snap.peers[1].name, "zebra");
    }

    #[test]
    fn reload_drops_revoked_peers() {
        let kp = libp2p_identity::Keypair::generate_ed25519();
        let pid = PeerId::from(kp.public()).to_base58();
        let (_td, peers_path, mut peers) = td_with_peer("phone", &pid);
        let mut state = DaemonState {
            peers: HashMap::new(),
            peers_toml: peers_path.clone(),
            peers_toml_mtime: None,
        };
        state.apply_peers(&peers);
        assert_eq!(state.peers.len(), 1);

        // User runs `bypass sync peer rm phone` → peers.toml shrinks.
        peers.remove("phone");
        // Force mtime change.
        std::thread::sleep(Duration::from_millis(10));
        peers.save(&peers_path).unwrap();

        state.reload_peers_if_changed().unwrap();
        assert!(state.peers.is_empty());
    }
}
