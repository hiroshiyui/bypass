// SPDX-License-Identifier: GPL-3.0-or-later

//! Daemon ↔ client Unix-socket protocol.
//!
//! Path resolution and multi-instance prevention per
//! [ADR-0017](../../../../doc/adr/0017-daemon-socket-location.md);
//! wire format per
//! [ADR-0018](../../../../doc/adr/0018-daemon-status-protocol.md).
//!
//! Two halves live here:
//! - **Daemon side**: [`bind_or_refuse_existing`] probes for a live
//!   daemon then binds; [`serve_status`] accepts connections in a loop
//!   and answers each with a [`StatusSnapshot`].
//! - **Client side**: [`query_status`] dials the socket and reads a
//!   single reply line.
//!
//! Unix-only. Windows support is out of scope for v1
//! (the daemon itself is `#[cfg(unix)]`).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Daemon-side error variants. Mostly conveyed via `anyhow` outside
/// this module, but the "already running" case warrants a typed
/// variant so the CLI dispatcher can map it to a specific exit code.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    #[error("bypass-sync daemon already running on {path} (close it first with `kill -TERM`)")]
    AlreadyRunning { path: PathBuf },

    #[error("socket I/O on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve the canonical socket path per ADR-0017.
///
/// 1. `$XDG_RUNTIME_DIR/bypass-sync.sock` if that env var points at a
///    non-empty directory.
/// 2. `$TMPDIR/bypass-<uid>-sync.sock`.
/// 3. `/tmp/bypass-<uid>-sync.sock`.
///
/// The `<uid>` suffix on the temp-dir variants is what makes the
/// path per-user on shared `/tmp`; `$XDG_RUNTIME_DIR` is already
/// per-user by definition.
pub fn default_socket_path() -> Result<PathBuf> {
    Ok(resolve_socket_path(
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("TMPDIR"),
        current_uid(),
    ))
}

/// Pure path-resolution rule used by [`default_socket_path`]. Lifted
/// out so unit tests don't have to mutate process-wide env vars.
fn resolve_socket_path(
    xdg_runtime_dir: Option<std::ffi::OsString>,
    tmpdir: Option<std::ffi::OsString>,
    uid: u32,
) -> PathBuf {
    if let Some(dir) = xdg_runtime_dir
        && !dir.is_empty()
    {
        return PathBuf::from(dir).join("bypass-sync.sock");
    }
    let name = format!("bypass-{uid}-sync.sock");
    let base = tmpdir
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join(name)
}

fn current_uid() -> u32 {
    // SAFETY: getuid is signal-safe and always succeeds.
    unsafe { libc_getuid() }
}

// Tiny wrapper so we don't pull `libc` as a crate just for one symbol.
// `std::os::unix::process::id` returns pid, not uid.
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// Bind a [`UnixListener`] at `path`, refusing if another daemon is
/// already serving on it. Cleans up a stale socket inode left behind
/// by a crashed daemon (`connect` returns `ECONNREFUSED`).
///
/// Returns the listener with `0600` perms on success.
pub async fn bind_or_refuse_existing(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        // Try to talk to whoever is on the other end. A successful
        // connect means there's a live daemon; refuse. Anything else
        // — `ECONNREFUSED` from a crashed daemon's orphan socket,
        // `ENOTSOCK` from a leftover regular file (macOS surfaces
        // this as `Other` with errno 38), `ENOENT` from a race —
        // means there is nothing live here, and we clear the path
        // before re-binding.
        match UnixStream::connect(path).await {
            Ok(_) => {
                return Err(SocketError::AlreadyRunning {
                    path: path.to_path_buf(),
                }
                .into());
            }
            Err(_) => {
                // Best-effort unlink; ignore `NotFound` (raced
                // ourselves to the cleanup). Anything else is a
                // permission / fs error we want to surface.
                if let Err(source) = std::fs::remove_file(path)
                    && source.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(SocketError::Io {
                        path: path.to_path_buf(),
                        source,
                    }
                    .into());
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create socket parent {}", parent.display()))?;
    }

    let listener = UnixListener::bind(path).map_err(|source| SocketError::Io {
        path: path.to_path_buf(),
        source,
    })?;

    // 0600 — only the daemon's uid can connect. Belt-and-braces on
    // Linux's runtime-dir (already 0700); load-bearing on macOS /tmp.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("chmod 0600 {}", path.display()))?;

    Ok(listener)
}

// ----- wire shape (ADR-0018) ------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Request {
    Status,
}

/// Daemon's response. Tagged `kind` so adding future ops is a
/// strict superset.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Status(StatusSnapshot),
    Error { error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub local_peer_id: String,
    pub listening_addrs: Vec<String>,
    pub peers: Vec<PeerStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    pub name: String,
    pub peer_id: String,
    pub discovered: bool,
    pub last_sync_action: Option<String>,
    pub last_sync_unix: Option<u64>,
}

// ----- daemon serve loop ----------------------------------------------

/// Accept-loop: spawn one short-lived task per accepted connection.
/// `snapshot` is called once per request — the daemon's state lock
/// stays in the closure's environment, never crosses an await point.
///
/// Returns when `listener` is closed or panics from the OS layer.
pub async fn serve_status<F>(listener: UnixListener, snapshot: F)
where
    F: Fn() -> StatusSnapshot + Send + Sync + 'static,
{
    let snapshot = std::sync::Arc::new(snapshot);
    loop {
        let (stream, _addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("bypass-sync: accept failed: {e}");
                // Brief backoff so we don't spin on a wedged listener.
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                continue;
            }
        };
        let snapshot = std::sync::Arc::clone(&snapshot);
        tokio::spawn(async move {
            if let Err(e) = handle_client(stream, &*snapshot).await {
                eprintln!("bypass-sync: client error: {e:#}");
            }
        });
    }
}

async fn handle_client<F>(stream: UnixStream, snapshot: &F) -> Result<()>
where
    F: Fn() -> StatusSnapshot,
{
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .context("read request line")?;
    let response = match serde_json::from_str::<Request>(line.trim()) {
        Ok(Request::Status) => Response::Status(snapshot()),
        Err(_) => Response::Error {
            error: "unknown op".into(),
        },
    };
    let mut bytes = serde_json::to_vec(&response).context("encode response")?;
    bytes.push(b'\n');
    write_half
        .write_all(&bytes)
        .await
        .context("write response")?;
    write_half.shutdown().await.ok();
    Ok(())
}

// ----- client query ---------------------------------------------------

/// Dial the daemon's socket and ask for a status snapshot. Returns a
/// friendly "daemon not running" error when the socket is absent or
/// refuses the connection (the most common client-side failure).
pub async fn query_status(path: &Path) -> Result<StatusSnapshot> {
    let stream = match UnixStream::connect(path).await {
        Ok(s) => s,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            bail!(
                "bypass-sync daemon not running ({} is unreachable); start it with `bypass sync daemon`",
                path.display()
            );
        }
        Err(source) => {
            return Err(SocketError::Io {
                path: path.to_path_buf(),
                source,
            }
            .into());
        }
    };
    let (read_half, mut write_half) = stream.into_split();
    let req = serde_json::to_vec(&Request::Status).expect("Request is serialisable");
    write_half.write_all(&req).await.context("send request")?;
    write_half
        .write_all(b"\n")
        .await
        .context("send request terminator")?;
    write_half.shutdown().await.ok();

    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    reader.read_line(&mut line).await.context("read response")?;
    let response: Response = serde_json::from_str(line.trim()).context("decode response")?;
    match response {
        Response::Status(s) => Ok(s),
        Response::Error { error } => Err(anyhow!("daemon: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture_snapshot() -> StatusSnapshot {
        StatusSnapshot {
            local_peer_id: "12D3KooW-local".into(),
            listening_addrs: vec!["/ip4/127.0.0.1/tcp/1234".into()],
            peers: vec![PeerStatus {
                name: "phone".into(),
                peer_id: "12D3KooW-phone".into(),
                discovered: true,
                last_sync_action: Some("FastForwarded".into()),
                last_sync_unix: Some(1_779_410_123),
            }],
        }
    }

    #[test]
    fn resolve_prefers_xdg_runtime_dir() {
        let p = resolve_socket_path(Some("/run/user/1000".into()), Some("/tmp".into()), 1000);
        assert_eq!(p, PathBuf::from("/run/user/1000/bypass-sync.sock"));
    }

    #[test]
    fn resolve_falls_back_to_tmpdir_with_uid_suffix() {
        let p = resolve_socket_path(None, Some("/var/folders/xx".into()), 501);
        assert_eq!(p, PathBuf::from("/var/folders/xx/bypass-501-sync.sock"));
    }

    #[test]
    fn resolve_falls_back_to_tmp_when_no_env_vars_set() {
        let p = resolve_socket_path(None, None, 42);
        assert_eq!(p, PathBuf::from("/tmp/bypass-42-sync.sock"));
    }

    #[test]
    fn resolve_treats_empty_xdg_runtime_dir_as_unset() {
        let p = resolve_socket_path(Some("".into()), Some("/tmp".into()), 7);
        assert_eq!(p, PathBuf::from("/tmp/bypass-7-sync.sock"));
    }

    #[tokio::test]
    async fn status_round_trips_over_a_real_socket() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("daemon.sock");
        let listener = bind_or_refuse_existing(&path).await.unwrap();
        let snapshot_fut = tokio::spawn(serve_status(listener, fixture_snapshot));

        let got = query_status(&path).await.unwrap();
        assert_eq!(got.local_peer_id, "12D3KooW-local");
        assert_eq!(got.peers.len(), 1);
        assert_eq!(got.peers[0].name, "phone");
        assert!(got.peers[0].discovered);
        assert_eq!(
            got.peers[0].last_sync_action.as_deref(),
            Some("FastForwarded")
        );

        snapshot_fut.abort();
    }

    #[tokio::test]
    async fn bind_refuses_when_a_listener_is_already_present() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("daemon.sock");
        let _live = bind_or_refuse_existing(&path).await.unwrap();
        // Spawn a serve loop so connect() succeeds (a bare listener
        // with no accepter would also succeed at the TCP-handshake
        // layer; this keeps the test stable).
        let serve = tokio::spawn(serve_status(_live, fixture_snapshot));
        let err = bind_or_refuse_existing(&path).await.unwrap_err();
        let downcast = err.downcast_ref::<SocketError>();
        assert!(
            matches!(downcast, Some(SocketError::AlreadyRunning { .. })),
            "expected AlreadyRunning, got {err:#}"
        );
        serve.abort();
    }

    #[tokio::test]
    async fn bind_recovers_from_a_stale_socket_inode() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("daemon.sock");
        // Create a regular file at the socket path — connect() will
        // fail with ECONNREFUSED (Linux) or similar. Our code path
        // unlinks and re-binds.
        std::fs::File::create(&path).unwrap();
        let listener = bind_or_refuse_existing(&path).await.unwrap();
        // Sanity-check that we now have a working listener.
        let snapshot_fut = tokio::spawn(serve_status(listener, fixture_snapshot));
        let _ = query_status(&path).await.unwrap();
        snapshot_fut.abort();
    }

    #[tokio::test]
    async fn unknown_op_returns_error_response() {
        let td = TempDir::new().unwrap();
        let path = td.path().join("daemon.sock");
        let listener = bind_or_refuse_existing(&path).await.unwrap();
        let serve = tokio::spawn(serve_status(listener, fixture_snapshot));

        // Write a malformed request directly.
        let stream = UnixStream::connect(&path).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        write_half.write_all(b"{\"op\":\"bogus\"}\n").await.unwrap();
        write_half.shutdown().await.ok();
        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"error\""), "got {line:?}");
        assert!(line.contains("unknown op"), "got {line:?}");

        serve.abort();
    }
}
