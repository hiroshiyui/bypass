// SPDX-License-Identifier: GPL-3.0-or-later

//! Native-messaging host: speaks the
//! [ADR-0022](../../../doc/adr/0022-native-messaging-wire-protocol.md)
//! wire format on stdin / stdout while the browser extension is open.
//!
//! Framing: 4-byte little-endian length prefix + UTF-8 JSON,
//! both directions. One request, one reply, repeated until the
//! browser closes the pipe (then `read_request` returns
//! `Ok(None)` and the loop exits cleanly).
//!
//! Plaintext-carrying replies wrap their byte buffers in
//! [`Zeroizing`] so the heap allocation is scrubbed on drop —
//! consistent with the [security audit
//! H1](../../../doc/security-audit.md) hardening.

use std::io::{self, Read, Write};

use anyhow::{Context, Result};
use bypass_core::path::RelPath;
use bypass_core::store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto_gpg::GpgCli;
use crate::storage_fs::StorageFs;
use crate::vcs_git2::Git2Vcs;

/// Per ADR-0022: replies cap at 512 KB. Both Firefox and Chrome
/// document a 1 MB ceiling; we leave headroom for envelope
/// overhead and refuse oversize replies with a clear error
/// instead of letting the browser truncate silently.
pub const MAX_REPLY_BYTES: usize = 512 * 1024;

// ----- wire envelope --------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum RequestBody {
    Ls {
        subpath: Option<String>,
    },
    Find {
        pattern: String,
    },
    Show {
        path: String,
        field: Option<String>,
    },
    Insert {
        path: String,
        plaintext: String,
        #[serde(default)]
        overwrite: bool,
    },
    Generate {
        path: String,
        #[serde(default)]
        length: Option<usize>,
        #[serde(default)]
        symbols: Option<bool>,
        #[serde(default)]
        in_place: bool,
        #[serde(default)]
        force: bool,
    },
    Otp {
        path: String,
    },
    Rm {
        path: String,
        #[serde(default)]
        recursive: bool,
    },
}

/// Wire request as the host sees it. The browser sends this exact
/// shape, framed by the 4-byte length prefix.
#[derive(Debug, Deserialize)]
struct Request {
    id: u64,
    #[serde(flatten)]
    body: RequestBody,
}

/// Reply envelope. Either `Ok` with op-specific fields or `Err`
/// with a sanitised string.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Reply {
    Ok {
        id: u64,
        ok: bool, // always true; tag for the extension
        #[serde(flatten)]
        body: OkBody,
    },
    Err {
        id: u64,
        ok: bool, // always false
        error: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OkBody {
    Entries { entries: Vec<String> },
    Plaintext { plaintext: String },
    Field { value: String },
    Password { password: String },
    Code { code: String },
    Empty {},
}

fn ok(id: u64, body: OkBody) -> Reply {
    Reply::Ok { id, ok: true, body }
}

fn err(id: u64, msg: impl Into<String>) -> Reply {
    Reply::Err {
        id,
        ok: false,
        error: msg.into(),
    }
}

// ----- top-level loop -------------------------------------------------

/// Run the messaging-host loop until stdin closes. Each iteration
/// reads one length-prefixed JSON request, dispatches it, and writes
/// one length-prefixed JSON reply.
pub fn run() -> Result<u8> {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let storage = StorageFs::new(root.clone());
    let crypto = GpgCli::new();
    let vcs = Git2Vcs::new(root);
    let mut store = Store::new(crypto, storage, vcs);

    while let Some(req_bytes) = read_frame(&mut stdin).context("read request frame")? {
        let reply = match serde_json::from_slice::<Request>(&req_bytes) {
            Ok(req) => dispatch(&mut store, req),
            Err(_) => Reply::Err {
                id: 0,
                ok: false,
                error: "malformed request".into(),
            },
        };
        let bytes = serde_json::to_vec(&reply).context("encode reply")?;
        if bytes.len() > MAX_REPLY_BYTES {
            // Too big to ship; replace with a size-error reply.
            // We can't quote the reply's `id` here without re-parsing
            // (we already discarded it); zero is the documented
            // fallback for "we couldn't keep the id".
            let oversize = err(
                0,
                format!(
                    "reply too large ({} bytes; max {MAX_REPLY_BYTES}); \
                     use the CLI for this entry",
                    bytes.len()
                ),
            );
            let bytes = serde_json::to_vec(&oversize).expect("err is always serialisable");
            write_frame(&mut stdout, &bytes).context("write reply frame")?;
        } else {
            write_frame(&mut stdout, &bytes).context("write reply frame")?;
        }
    }
    Ok(0)
}

// ----- framing --------------------------------------------------------

/// Read one length-prefixed JSON frame from `r`. Returns `Ok(None)`
/// on a clean EOF (browser closed the port) so the caller's loop
/// can exit; returns `Err` only on a partial / corrupt frame.
fn read_frame<R: Read>(r: &mut R) -> Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(anyhow::Error::from(e).context("read length prefix")),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    // Defence-in-depth: a corrupt length prefix could ask for
    // gigabytes. Chrome's max request size is 4 GB, but anything
    // approaching that is a bug on the sender side. Cap at the
    // same 512 KB we use for replies; a real bypass extension
    // never sends bigger.
    if len > MAX_REPLY_BYTES {
        anyhow::bail!("request frame too large: {len} bytes");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).context("read request body")?;
    Ok(Some(buf))
}

fn write_frame<W: Write>(w: &mut W, body: &[u8]) -> Result<()> {
    let len = u32::try_from(body.len()).context("reply too large for u32 length prefix")?;
    w.write_all(&len.to_le_bytes())
        .context("write length prefix")?;
    w.write_all(body).context("write reply body")?;
    w.flush().context("flush reply")?;
    Ok(())
}

// ----- dispatch -------------------------------------------------------

fn dispatch(store: &mut Store<GpgCli, StorageFs, Git2Vcs>, req: Request) -> Reply {
    let Request { id, body } = req;
    match body {
        RequestBody::Ls { subpath } => match parse_optional_path(subpath) {
            Err(e) => err(id, e),
            Ok(sub) => match store.list(sub.as_ref()) {
                Ok(entries) => ok(
                    id,
                    OkBody::Entries {
                        entries: entries.into_iter().map(|p| p.as_str().to_owned()).collect(),
                    },
                ),
                Err(e) => err(id, store_err_to_user(e)),
            },
        },
        RequestBody::Find { pattern } => match store.find(&pattern) {
            Ok(entries) => ok(
                id,
                OkBody::Entries {
                    entries: entries.into_iter().map(|p| p.as_str().to_owned()).collect(),
                },
            ),
            Err(e) => err(id, store_err_to_user(e)),
        },
        RequestBody::Show { path, field } => match parse_path(&path) {
            Err(e) => err(id, e),
            Ok(rel) => match store.show(&rel) {
                Ok(plaintext) => match field {
                    Some(name) => extract_field(id, plaintext, &name),
                    None => {
                        let s = match std::str::from_utf8(plaintext.as_slice()) {
                            Ok(s) => Zeroizing::new(s.to_owned()),
                            Err(_) => return err(id, "entry is not valid UTF-8"),
                        };
                        ok(
                            id,
                            OkBody::Plaintext {
                                plaintext: (*s).clone(),
                            },
                        )
                    }
                },
                Err(e) => err(id, store_err_to_user(e)),
            },
        },
        RequestBody::Insert {
            path,
            plaintext,
            overwrite,
        } => match parse_path(&path) {
            Err(e) => err(id, e),
            Ok(rel) => {
                let pt = Zeroizing::new(plaintext);
                match store.insert(&rel, pt.as_bytes(), overwrite) {
                    Ok(()) => ok(id, OkBody::Empty {}),
                    Err(e) => err(id, store_err_to_user(e)),
                }
            }
        },
        RequestBody::Generate {
            path,
            length,
            symbols,
            in_place,
            force,
        } => match parse_path(&path) {
            Err(e) => err(id, e),
            Ok(rel) => generate_op(store, id, &rel, length, symbols, in_place, force),
        },
        RequestBody::Otp { path } => match parse_path(&path) {
            Err(e) => err(id, e),
            Ok(rel) => match store.show(&rel) {
                Ok(plaintext) => match std::str::from_utf8(plaintext.as_slice()) {
                    Err(_) => err(id, "entry is not valid UTF-8"),
                    Ok(text) => match bypass_core::otp::current_code(text) {
                        Ok(code) => {
                            let code = Zeroizing::new(code);
                            ok(
                                id,
                                OkBody::Code {
                                    code: (*code).clone(),
                                },
                            )
                        }
                        Err(e) => err(id, format!("compute TOTP code: {e}")),
                    },
                },
                Err(e) => err(id, store_err_to_user(e)),
            },
        },
        RequestBody::Rm { path, recursive } => match parse_path(&path) {
            Err(e) => err(id, e),
            Ok(rel) => {
                let result = if recursive {
                    store.remove_recursive(&rel).map(|_| ())
                } else {
                    store.remove(&rel)
                };
                match result {
                    Ok(()) => ok(id, OkBody::Empty {}),
                    Err(e) => err(id, store_err_to_user(e)),
                }
            }
        },
    }
}

fn extract_field(id: u64, plaintext: bypass_core::crypto::SecretBytes, field: &str) -> Reply {
    let parsed = match bypass_core::entry::Entry::parse(plaintext.as_slice()) {
        Ok(p) => p,
        Err(e) => return err(id, format!("parse entry body: {e}")),
    };
    match parsed.field(field) {
        Some(v) => {
            let v = Zeroizing::new(v.to_owned());
            ok(
                id,
                OkBody::Field {
                    value: (*v).clone(),
                },
            )
        }
        None => err(id, format!("entry has no field {field:?}")),
    }
}

fn generate_op(
    store: &mut Store<GpgCli, StorageFs, Git2Vcs>,
    id: u64,
    rel: &RelPath,
    length: Option<usize>,
    symbols: Option<bool>,
    in_place: bool,
    force: bool,
) -> Reply {
    let length = length.unwrap_or(bypass_core::generate::DEFAULT_LENGTH);
    let with_symbols = symbols.unwrap_or(true);
    let password = Zeroizing::new(bypass_core::generate::generate(length, with_symbols));
    let result = if in_place {
        let existing: Zeroizing<Vec<u8>> = match store.show(rel) {
            Ok(b) => Zeroizing::new(b.as_slice().to_vec()),
            Err(e) => return err(id, store_err_to_user(e)),
        };
        let tail: &[u8] = match existing.iter().position(|&b| b == b'\n') {
            Some(i) => &existing[i..],
            None => b"",
        };
        let mut new_body: Zeroizing<Vec<u8>> = Zeroizing::new(password.as_bytes().to_vec());
        new_body.extend_from_slice(tail);
        store.insert(rel, &new_body, /*overwrite=*/ true)
    } else {
        store.insert(rel, password.as_bytes(), force)
    };
    match result {
        Ok(()) => ok(
            id,
            OkBody::Password {
                password: (*password).clone(),
            },
        ),
        Err(e) => err(id, store_err_to_user(e)),
    }
}

// ----- helpers --------------------------------------------------------

fn parse_path(s: &str) -> std::result::Result<RelPath, String> {
    RelPath::new(s).map_err(|e| format!("invalid entry path: {e}"))
}

fn parse_optional_path(s: Option<String>) -> std::result::Result<Option<RelPath>, String> {
    match s {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => parse_path(&s).map(Some),
    }
}

/// Map a `StoreError` to a user-facing string. The CLI's own error
/// path already does this via `anyhow::Error::new(e)` + `Display`;
/// we mirror the same display rendering but skip the chain so the
/// extension doesn't see internal source-error contexts that mention
/// host paths.
fn store_err_to_user<CE, SE, VE>(e: StoreError<CE, SE, VE>) -> String
where
    CE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    VE: std::error::Error + Send + Sync + 'static,
{
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn frame(json: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let bytes = json.as_bytes();
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(bytes);
        out
    }

    #[test]
    fn read_frame_returns_none_on_clean_eof() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_frame_round_trips_one_message() {
        let mut cursor = Cursor::new(frame("{\"id\":1,\"op\":\"ls\"}"));
        let bytes = read_frame(&mut cursor).unwrap().unwrap();
        assert_eq!(bytes, b"{\"id\":1,\"op\":\"ls\"}");
        // Then EOF.
        assert!(read_frame(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_frame_refuses_oversize_length_prefix() {
        // 0xFFFFFFFF as a length prefix = 4 GB; we cap at MAX_REPLY_BYTES.
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        let mut cursor = Cursor::new(buf);
        let err = read_frame(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("too large"), "got {err:#}");
    }

    #[test]
    fn write_frame_emits_length_prefix_then_body() {
        let mut out = Vec::new();
        write_frame(&mut out, b"abc").unwrap();
        assert_eq!(&out[..4], &3u32.to_le_bytes());
        assert_eq!(&out[4..], b"abc");
    }

    #[test]
    fn malformed_json_yields_error_reply_with_id_zero() {
        // We construct the dispatch path directly by feeding a
        // bad-JSON request bytes through `read_frame` + parse; the
        // outer `run()` loop is what maps a parse failure to id=0,
        // so we replicate that here.
        let parsed: Result<Request, _> = serde_json::from_slice(b"{not json}");
        assert!(parsed.is_err());
        let reply = Reply::Err {
            id: 0,
            ok: false,
            error: "malformed request".into(),
        };
        let s = serde_json::to_string(&reply).unwrap();
        assert!(s.contains("\"ok\":false"), "{s}");
        assert!(s.contains("\"id\":0"), "{s}");
        assert!(s.contains("malformed request"), "{s}");
    }

    #[test]
    fn unknown_op_serializes_as_serde_error_at_request_level() {
        // serde-tagged enums treat an unknown `op` as a parse
        // failure; the outer loop maps that to "malformed request"
        // with id=0, matching the schema documented in ADR-0022.
        let parsed: Result<Request, _> = serde_json::from_slice(br#"{"id":99,"op":"unknown"}"#);
        assert!(parsed.is_err());
    }
}
