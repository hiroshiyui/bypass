// SPDX-License-Identifier: GPL-3.0-or-later

//! Pinned-peer table.
//!
//! Lives at `$XDG_CONFIG_HOME/bypass/peers.toml` per
//! [ADR-0012](../../../../doc/adr/0012-pake-spake2.md). One file, list
//! of records; each record captures the libp2p peer id, the peer's
//! Noise static public key (carried as opaque bytes — the Noise key is
//! just the peer's public-key bytes for Ed25519-backed libp2p peers
//! anyway), an operator-supplied friendly name, and `paired_at`.
//!
//! Atomic writes (tempfile + rename) match the identity-file pattern
//! so a crash mid-update doesn't leave partial state.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use libp2p_identity::PeerId;
use serde::{Deserialize, Serialize};

/// A single pinned peer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerRecord {
    /// Stable name the user assigned during pairing (e.g. `laptop`).
    pub name: String,
    /// libp2p peer id, base58-encoded.
    pub peer_id: String,
    /// Peer's Noise static public key, hex-encoded. For Ed25519 libp2p
    /// peers this is the same key material as the peer id derives from,
    /// but kept explicit so a future algorithm migration doesn't break
    /// the table format.
    pub noise_static_key: String,
    /// RFC 3339 wall-clock timestamp of when this entry was added.
    pub paired_at: String,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PeersFile {
    #[serde(default, rename = "peer")]
    peers: Vec<PeerRecord>,
}

/// In-memory representation of `peers.toml`.
#[derive(Debug, Default, Clone)]
pub struct Peers {
    inner: PeersFile,
}

impl Peers {
    /// Resolve the canonical on-disk location:
    /// `$XDG_CONFIG_HOME/bypass/peers.toml`.
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir().context(
            "cannot resolve $XDG_CONFIG_HOME (or its fallback); set the variable manually",
        )?;
        Ok(dir.join("bypass").join("peers.toml"))
    }

    /// Load the peers table. Returns an empty table if the file
    /// doesn't exist (a freshly-installed device has no paired peers).
    pub fn load(path: &Path) -> Result<Self> {
        let bytes = match fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(anyhow::Error::from(source).context(format!("read {}", path.display())));
            }
        };
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
        let inner: PeersFile =
            toml::from_str(text).with_context(|| format!("parse {} as TOML", path.display()))?;
        Ok(Self { inner })
    }

    /// Atomically write the table to disk. Creates the parent
    /// directory if missing.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create peers directory {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(&self.inner).context("serialise peers to TOML")?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let tmp = parent.join(format!(
            ".{}.tmp.{}",
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("peers.toml"),
            std::process::id()
        ));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Peers file isn't strictly a secret (it holds public
            // peer-ids), but mode 0600 keeps it consistent with the
            // identity key sitting next to it.
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .with_context(|| format!("create tempfile {}", tmp.display()))?;
        let write_res = f.write_all(text.as_bytes()).and_then(|()| f.sync_all());
        drop(f);
        if let Err(e) = write_res {
            let _ = fs::remove_file(&tmp);
            return Err(anyhow::Error::from(e).context(format!("write {}", tmp.display())));
        }
        if let Err(e) = fs::rename(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(anyhow::Error::from(e).context(format!(
                "rename {} → {}",
                tmp.display(),
                path.display()
            )));
        }
        Ok(())
    }

    pub fn records(&self) -> &[PeerRecord] {
        &self.inner.peers
    }

    pub fn find(&self, peer_id: &PeerId) -> Option<&PeerRecord> {
        let needle = peer_id.to_base58();
        self.inner.peers.iter().find(|r| r.peer_id == needle)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&PeerRecord> {
        self.inner.peers.iter().find(|r| r.name == name)
    }

    /// Append a record. If a peer with the same `peer_id` already
    /// exists, the existing entry is replaced. Returns whether the
    /// inserted record was new (`true`) or replaced an existing one
    /// (`false`).
    pub fn upsert(&mut self, record: PeerRecord) -> bool {
        if let Some(pos) = self
            .inner
            .peers
            .iter()
            .position(|r| r.peer_id == record.peer_id)
        {
            self.inner.peers[pos] = record;
            false
        } else {
            self.inner.peers.push(record);
            true
        }
    }

    /// Remove the record matching `name`. Returns the removed record,
    /// or `None` if no such peer.
    pub fn remove(&mut self, name: &str) -> Option<PeerRecord> {
        let pos = self.inner.peers.iter().position(|r| r.name == name)?;
        Some(self.inner.peers.remove(pos))
    }

    /// Clear every paired peer. Used by
    /// `bypass sync identity rotate` per ADR-0015.
    pub fn clear(&mut self) {
        self.inner.peers.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.inner.peers.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rec(name: &str, peer_id: &str) -> PeerRecord {
        PeerRecord {
            name: name.into(),
            peer_id: peer_id.into(),
            noise_static_key: "deadbeef".into(),
            paired_at: "2026-05-22T12:00:00Z".into(),
        }
    }

    #[test]
    fn load_returns_empty_when_file_missing() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("peers.toml");
        let peers = Peers::load(&p).unwrap();
        assert!(peers.is_empty());
        assert!(peers.records().is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("peers.toml");
        let mut peers = Peers::default();
        peers.upsert(rec("laptop", "12D3KooWLaptop"));
        peers.upsert(rec("phone", "12D3KooWPhone"));
        peers.save(&p).unwrap();
        let again = Peers::load(&p).unwrap();
        assert_eq!(again.records().len(), 2);
        assert_eq!(again.records()[0].name, "laptop");
    }

    #[test]
    fn upsert_replaces_existing_peer_with_same_id() {
        let mut peers = Peers::default();
        assert!(peers.upsert(rec("old-name", "12D3KooWSame")));
        // Same peer_id, different name → replace, not append.
        assert!(!peers.upsert(rec("new-name", "12D3KooWSame")));
        assert_eq!(peers.records().len(), 1);
        assert_eq!(peers.records()[0].name, "new-name");
    }

    #[test]
    fn remove_by_name_works_and_misses_silently() {
        let mut peers = Peers::default();
        peers.upsert(rec("laptop", "12D3KooWLaptop"));
        assert!(peers.remove("laptop").is_some());
        assert!(peers.is_empty());
        // Removing again is a no-op.
        assert!(peers.remove("laptop").is_none());
    }

    #[test]
    fn clear_drops_every_peer() {
        let mut peers = Peers::default();
        peers.upsert(rec("a", "12D3KooWA"));
        peers.upsert(rec("b", "12D3KooWB"));
        peers.clear();
        assert!(peers.is_empty());
    }

    #[test]
    fn find_by_name_returns_match() {
        let mut peers = Peers::default();
        peers.upsert(rec("phone", "12D3KooWPhone"));
        assert!(peers.find_by_name("phone").is_some());
        assert!(peers.find_by_name("absent").is_none());
    }

    #[test]
    fn empty_table_serialises_as_empty_array() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("peers.toml");
        let peers = Peers::default();
        peers.save(&p).unwrap();
        let text = fs::read_to_string(&p).unwrap();
        // toml-rs renders an empty array-of-tables as no `[[peer]]`
        // sections at all, which is fine — reloading it round-trips
        // to an empty Peers above.
        assert!(text.is_empty() || !text.contains("[[peer]]"));
    }

    #[test]
    #[cfg(unix)]
    fn save_writes_0600_perms() {
        use std::os::unix::fs::PermissionsExt;
        let td = TempDir::new().unwrap();
        let p = td.path().join("peers.toml");
        let peers = Peers::default();
        peers.save(&p).unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
