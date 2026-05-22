// SPDX-License-Identifier: GPL-3.0-or-later

//! Per-device identity keypair.
//!
//! See [ADR-0015](../../../../doc/adr/0015-device-identity-key.md):
//!
//! - Ed25519 keypair generated via `libp2p_identity`.
//! - Stored at `$XDG_CONFIG_HOME/bypass/identity.key` with `0600`
//!   permissions.
//! - On-disk format is libp2p's protobuf encoding.
//! - Loader refuses files with permissions wider than `0600` on Unix.
//! - Writes are atomic (tempfile + rename).

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use libp2p_identity::{Keypair, PeerId};

/// Resolve the canonical on-disk location for this device's identity
/// key: `$XDG_CONFIG_HOME/bypass/identity.key`, with the usual
/// `dirs::config_dir()` fallback for unset XDG.
pub fn default_path() -> Result<PathBuf> {
    let dir = dirs::config_dir()
        .context("cannot resolve $XDG_CONFIG_HOME (or its fallback); set the variable manually")?;
    Ok(dir.join("bypass").join("identity.key"))
}

/// Load the identity at `path`, generating a fresh Ed25519 keypair and
/// writing it if the file does not yet exist. The returned [`Keypair`]
/// is the device's stable identity for [`bypass-sync`] purposes.
pub fn load_or_generate(path: &Path) -> Result<Keypair> {
    match load(path) {
        Ok(kp) => Ok(kp),
        Err(LoadError::NotFound) => {
            let kp = Keypair::generate_ed25519();
            save(path, &kp)
                .with_context(|| format!("write fresh identity to {}", path.display()))?;
            Ok(kp)
        }
        Err(e) => Err(anyhow::Error::new(e)),
    }
}

/// Force-generate a new keypair and overwrite the existing identity
/// file. Used by `bypass sync identity rotate`. The caller is
/// responsible for invalidating `peers.toml` afterwards (rotation
/// breaks every pinned peer relationship).
pub fn rotate(path: &Path) -> Result<Keypair> {
    let kp = Keypair::generate_ed25519();
    save(path, &kp).with_context(|| format!("write rotated identity to {}", path.display()))?;
    Ok(kp)
}

/// Errors raised while loading the identity file.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("identity file not found")]
    NotFound,

    #[error(
        "identity file has permissions {mode:#o}; expected 0600 (chmod 600 the file or delete it to regenerate)"
    )]
    WidePerms { mode: u32 },

    #[error("I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("malformed identity file (not a libp2p protobuf keypair): {0}")]
    Malformed(String),
}

/// Load an existing identity. Returns [`LoadError::NotFound`] if the
/// file does not exist (callers wanting create-on-miss should use
/// [`load_or_generate`]).
pub fn load(path: &Path) -> Result<Keypair, LoadError> {
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(LoadError::NotFound),
        Err(source) => {
            return Err(LoadError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            return Err(LoadError::WidePerms { mode });
        }
    }
    let _ = meta;
    let bytes = fs::read(path).map_err(|source| LoadError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Keypair::from_protobuf_encoding(&bytes).map_err(|e| LoadError::Malformed(e.to_string()))
}

/// Write `kp` to `path` atomically and (on Unix) with mode `0600`. The
/// containing directory is created if missing.
pub fn save(path: &Path, kp: &Keypair) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create identity directory {}", parent.display()))?;
    }
    let bytes = kp
        .to_protobuf_encoding()
        .context("serialise identity to protobuf")?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(
        ".{}.tmp.{}",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("identity"),
        std::process::id()
    ));

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("create tempfile {}", tmp.display()))?;
    let write_res = f.write_all(&bytes).and_then(|()| f.sync_all());
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

/// Convenience: derive this keypair's libp2p `PeerId`.
pub fn peer_id(kp: &Keypair) -> PeerId {
    PeerId::from(kp.public())
}

/// Reject rotation requests that lack the `--confirm` flag. Centralised
/// so the CLI dispatch arm and any future programmatic caller share the
/// same fail-loud behaviour.
pub fn ensure_rotate_confirmed(confirm: bool) -> Result<()> {
    if !confirm {
        bail!(
            "`bypass sync identity rotate` is destructive (every paired peer relationship \
             is lost; you must re-pair). Re-run with `--confirm` to acknowledge."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn load_or_generate_creates_a_fresh_keypair_on_first_call() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("identity.key");
        assert!(!path.exists());
        let kp = load_or_generate(&path).unwrap();
        assert!(path.exists(), "identity file must be created");
        // Round-trip: reloading yields the same key.
        let again = load(&path).unwrap();
        assert_eq!(peer_id(&kp), peer_id(&again));
    }

    #[test]
    fn save_writes_0600_perms_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let td = TempDir::new().unwrap();
            let path = td.path().join("identity.key");
            let kp = Keypair::generate_ed25519();
            save(&path, &kp).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "wrote with mode {mode:#o}, expected 0o600");
        }
    }

    #[test]
    fn load_refuses_wide_perms_on_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let td = TempDir::new().unwrap();
            let path = td.path().join("identity.key");
            let kp = Keypair::generate_ed25519();
            save(&path, &kp).unwrap();
            // chmod to a group-readable mode.
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
            match load(&path) {
                Err(LoadError::WidePerms { mode }) => assert_eq!(mode, 0o640),
                other => panic!("expected WidePerms, got {other:?}"),
            }
        }
    }

    #[test]
    fn load_returns_not_found_for_missing_file() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("identity.key");
        match load(&path) {
            Err(LoadError::NotFound) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn rotate_overwrites_with_a_fresh_keypair() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("identity.key");
        let first = load_or_generate(&path).unwrap();
        let second = rotate(&path).unwrap();
        assert_ne!(
            peer_id(&first),
            peer_id(&second),
            "rotation must change the peer id"
        );
        // Disk state matches the rotated key.
        assert_eq!(peer_id(&second), peer_id(&load(&path).unwrap()));
    }

    #[test]
    fn ensure_rotate_confirmed_requires_the_flag() {
        assert!(ensure_rotate_confirmed(false).is_err());
        assert!(ensure_rotate_confirmed(true).is_ok());
    }
}
