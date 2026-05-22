// SPDX-License-Identifier: GPL-3.0-or-later

mod audit;
mod cli;
mod clipboard;
mod crypto_gpg;
mod doctor;
mod edit;
mod extensions;
mod storage_fs;
mod sync;
mod tree;
mod vcs_git2;

use std::io::{self, Read, Write};
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use bypass_core::crypto::KeyId;
use bypass_core::generate::{self, DEFAULT_LENGTH};
use bypass_core::path::RelPath;
use bypass_core::store::Store;
use clap::Parser;
use zeroize::Zeroizing;

use crate::clipboard::DEFAULT_CLEAR_SECS;

use crate::cli::{Cli, Command};
use crate::crypto_gpg::GpgCli;
use crate::storage_fs::StorageFs;
use crate::vcs_git2::Git2Vcs;

fn main() -> ExitCode {
    match dispatch() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("bypass: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn dispatch() -> Result<u8> {
    let args = Cli::parse();
    match args.command {
        Command::Otp { path, clip } => {
            let entry = parse_entry(&path)?;
            let store = open_store()?;
            let plaintext = store.show(&entry).map_err(map_store_err)?;
            let text =
                std::str::from_utf8(plaintext.as_slice()).context("entry is not valid UTF-8")?;
            // Wrap the TOTP code in `Zeroizing` so the heap String holding
            // the six digits scrubs on drop (security audit H5; TOTP codes
            // are short-lived but still secrets).
            let code: Zeroizing<String> =
                Zeroizing::new(bypass_core::otp::current_code(text).context("compute TOTP code")?);
            if clip {
                clipboard::copy_and_auto_clear(code.as_bytes(), DEFAULT_CLEAR_SECS)?;
            } else {
                println!("{}", &*code);
            }
            Ok(0)
        }
        Command::Doctor => Ok(doctor::run() as u8),
        Command::Log { path } => {
            let entry = path.as_deref().map(parse_entry).transpose()?;
            let store = open_store()?;
            let commits = store.log(entry.as_ref()).map_err(map_store_err)?;
            for c in &commits {
                let short = c.id.get(..7).unwrap_or(c.id.as_str());
                println!("{short} {}", c.summary);
            }
            Ok(0)
        }
        Command::Init { gpg_ids } => {
            let root = StorageFs::resolve_default_root().context("resolve store root")?;
            let mut store = open_store()?;
            let keys: Vec<KeyId> = gpg_ids.into_iter().map(KeyId::new).collect();
            store.init(&keys).map_err(map_store_err)?;
            // Register the merge driver referenced by `.gitattributes`
            // (see ADR-0011). Idempotent; safe to run on every init.
            sync::merge_driver::register_in_git_config(&root)?;
            Ok(0)
        }
        Command::Insert {
            path,
            force,
            multiline,
        } => {
            let entry = parse_entry(&path)?;
            let plaintext = read_secret_from_stdin(multiline)?;
            let mut store = open_store()?;
            store
                .insert(&entry, plaintext.as_slice(), force)
                .map_err(map_store_err)?;
            Ok(0)
        }
        Command::Show { path, field, clip } => {
            let entry = parse_entry(&path)?;
            let store = open_store()?;
            let plaintext = store.show(&entry).map_err(map_store_err)?;
            // Output bytes depending on whether a field was requested.
            // Wrapped in `Zeroizing` so the heap allocation backing the
            // copy is scrubbed on drop (security audit: H1).
            let output: Zeroizing<Vec<u8>> = Zeroizing::new(match field.as_deref() {
                Some(name) => {
                    let parsed = bypass_core::entry::Entry::parse(plaintext.as_slice())
                        .context("parse entry body")?;
                    let value = parsed
                        .field(name)
                        .ok_or_else(|| anyhow!("entry has no field {name:?}"))?;
                    value.as_bytes().to_vec()
                }
                None => plaintext.as_slice().to_vec(),
            });
            if clip {
                // Whole-entry copy is meaningless: a multi-line entry would
                // paste with key:value rows attached. So `-c` without a
                // field copies just the first line (the password);
                // with a field it copies the field value.
                let to_copy: &[u8] = if field.is_some() {
                    &output
                } else {
                    output
                        .iter()
                        .position(|&b| b == b'\n')
                        .map(|i| &output[..i])
                        .unwrap_or(&output)
                };
                clipboard::copy_and_auto_clear(to_copy, DEFAULT_CLEAR_SECS)?;
            } else {
                io::stdout().write_all(&output).context("write stdout")?;
                // Append a trailing newline if the output didn't already
                // end with one, matching pass.
                if output.last() != Some(&b'\n') {
                    let _ = writeln!(io::stdout());
                }
            }
            Ok(0)
        }
        Command::Ls { subpath } => {
            let store = open_store()?;
            let sub = subpath.as_deref().map(parse_entry).transpose()?;
            let entries = store.list(sub.as_ref()).map_err(map_store_err)?;
            let (display_entries, header_owned);
            let header: &str = match &sub {
                Some(p) => {
                    let prefix = format!("{}/", p.as_str());
                    display_entries = entries
                        .iter()
                        .filter_map(|e| {
                            e.as_str()
                                .strip_prefix(&prefix)
                                .and_then(|s| RelPath::new(s).ok())
                        })
                        .collect::<Vec<_>>();
                    header_owned = format!("{}/", p.as_str());
                    &header_owned
                }
                None => {
                    display_entries = entries;
                    "Password Store"
                }
            };
            print!("{}", tree::render(&display_entries, header));
            Ok(0)
        }
        Command::Find { pattern } => {
            let store = open_store()?;
            let entries = store.find(&pattern).map_err(map_store_err)?;
            for e in entries {
                println!("{e}");
            }
            Ok(0)
        }
        Command::Rm { path, recursive } => {
            let target = parse_entry(&path)?;
            let mut store = open_store()?;
            if recursive {
                let removed = store.remove_recursive(&target).map_err(map_store_err)?;
                for entry in &removed {
                    eprintln!("removed {entry}");
                }
            } else {
                store.remove(&target).map_err(map_store_err)?;
                eprintln!("removed {target}");
            }
            Ok(0)
        }
        Command::Edit { path } => {
            let entry = parse_entry(&path)?;
            let mut store = open_store()?;
            edit::run(&mut store, &entry)?;
            Ok(0)
        }
        Command::Generate {
            path,
            length,
            no_symbols,
            in_place,
            force,
            clip,
        } => {
            let entry = parse_entry(&path)?;
            let length = length.unwrap_or(DEFAULT_LENGTH);
            // Wrap the generated password in `Zeroizing` so it's scrubbed
            // on drop (security audit: H1). `generate` returns a plain
            // `String`; the wrap covers it from that point on.
            let password: Zeroizing<String> =
                Zeroizing::new(generate::generate(length, !no_symbols));
            let mut store = open_store()?;
            if in_place {
                // Replace only the first line; keep the rest of the body.
                // Both intermediate buffers hold plaintext — `Zeroizing`
                // them per audit H1.
                let existing: Zeroizing<Vec<u8>> = Zeroizing::new(match store.show(&entry) {
                    Ok(b) => b.as_slice().to_vec(),
                    Err(e) => return Err(map_store_err(e)),
                });
                let tail: &[u8] = match existing.iter().position(|&b| b == b'\n') {
                    Some(i) => &existing[i..],
                    None => b"",
                };
                let mut new_body: Zeroizing<Vec<u8>> = Zeroizing::new(password.as_bytes().to_vec());
                new_body.extend_from_slice(tail);
                store
                    .insert(&entry, &new_body, /*overwrite=*/ true)
                    .map_err(map_store_err)?;
            } else {
                store
                    .insert(&entry, password.as_bytes(), force)
                    .map_err(map_store_err)?;
            }
            if clip {
                clipboard::copy_and_auto_clear(password.as_bytes(), DEFAULT_CLEAR_SECS)?;
            } else {
                // `password` is `Zeroizing<String>`; deref to `&str` for
                // formatting so we don't borrow it as `Zeroizing` (which
                // doesn't impl `Display`).
                println!("{}", &*password);
            }
            Ok(0)
        }
        Command::Cp { from, to, force } => {
            let from_entry = parse_entry(&from)?;
            let to_entry = parse_entry(&to)?;
            let mut store = open_store()?;
            store
                .copy(&from_entry, &to_entry, force)
                .map_err(map_store_err)?;
            eprintln!("copied {from_entry} to {to_entry}");
            Ok(0)
        }
        Command::Completion { shell } => {
            let mut cmd = <cli::Cli as clap::CommandFactory>::command();
            clap_complete::generate(shell, &mut cmd, "bypass", &mut io::stdout());
            Ok(0)
        }
        Command::Man => {
            let cmd = <cli::Cli as clap::CommandFactory>::command();
            let man = clap_mangen::Man::new(cmd);
            let mut buf: Vec<u8> = Vec::new();
            man.render(&mut buf).context("render man page")?;
            io::stdout().write_all(&buf).context("write man page")?;
            Ok(0)
        }
        Command::ClipboardSet { seconds } => {
            clipboard::run_daemon(seconds)?;
            Ok(0)
        }
        Command::MergeTakeTheirs {
            ancestor,
            ours,
            theirs,
            path,
            marker_size: _,
        } => sync::merge_driver::take_theirs(
            std::path::Path::new(&ancestor),
            std::path::Path::new(&ours),
            std::path::Path::new(&theirs),
            &path,
        ),
        Command::Ext { name, args } => extensions::dispatch(&name, &args),
        Command::Sync { force, sub } => match sub {
            None => sync(force),
            Some(cli::SyncCmd::Identity {
                action: cli::SyncIdentityCmd::Rotate { confirm },
            }) => sync_identity_rotate(confirm),
            Some(cli::SyncCmd::Pair {
                show,
                enter,
                name,
                addr,
            }) => sync_pair(show, enter, name, addr),
            Some(cli::SyncCmd::Daemon { action }) => sync_daemon_dispatch(action),
            Some(cli::SyncCmd::Status { json }) => sync_status(json),
            Some(cli::SyncCmd::Peer {
                action: cli::SyncPeerCmd::Rm { name, yes },
            }) => sync_peer_rm(&name, yes),
        },
        Command::Audit => audit_cmd(),
        Command::Git { args } => {
            let root = StorageFs::resolve_default_root().context("resolve store root")?;
            let status = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(&args)
                .status()
                .with_context(|| format!("spawn `git` against {}", root.display()))?;
            Ok(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1))
        }
        Command::Mv { from, to, force } => {
            let from_entry = parse_entry(&from)?;
            let to_entry = parse_entry(&to)?;
            let mut store = open_store()?;
            store
                .rename(&from_entry, &to_entry, force)
                .map_err(map_store_err)?;
            eprintln!("renamed {from_entry} to {to_entry}");
            Ok(0)
        }
    }
}

fn open_store() -> Result<Store<GpgCli, StorageFs, Git2Vcs>> {
    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let storage = StorageFs::new(root.clone());
    let crypto = GpgCli::new();
    let vcs = Git2Vcs::new(root);
    Ok(Store::new(crypto, storage, vcs))
}

fn parse_entry(s: &str) -> Result<RelPath> {
    RelPath::new(s).map_err(|e| anyhow!("invalid entry path: {e}"))
}

fn read_secret_from_stdin(multiline: bool) -> Result<Zeroizing<Vec<u8>>> {
    // Every intermediate buffer that may hold plaintext is wrapped in
    // `Zeroizing` so its heap allocation is scrubbed on drop (security
    // audit H2). `rpassword::prompt_password` allocates a `String`
    // we can't zeroize at the source, so we zeroize the bytes we
    // *copy out of it* before the `String` is dropped.
    if multiline {
        let mut buf: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::new());
        io::stdin().read_to_end(&mut buf).context("read stdin")?;
        Ok(buf)
    } else {
        // For non-interactive callers (pipes), read a single line as-is.
        // For interactive callers, prompt twice with echo off.
        if !atty_stdin() {
            let mut line: Zeroizing<String> = Zeroizing::new(String::new());
            io::stdin().read_line(&mut line).context("read stdin")?;
            // Strip the trailing newline that read_line includes, mirroring
            // how a user pressing Enter on a TTY would not store the newline.
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(Zeroizing::new(line.as_bytes().to_vec()))
        } else {
            let a: Zeroizing<String> = Zeroizing::new(
                rpassword::prompt_password("Enter password: ").context("read password")?,
            );
            let b: Zeroizing<String> = Zeroizing::new(
                rpassword::prompt_password("Retype password: ").context("confirm password")?,
            );
            if *a != *b {
                bail!("passwords do not match");
            }
            Ok(Zeroizing::new(a.as_bytes().to_vec()))
        }
    }
}

/// Best-effort stdin-is-a-TTY check using `IsTerminal` from std.
fn atty_stdin() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

fn map_store_err<CE, SE, VE>(e: bypass_core::store::StoreError<CE, SE, VE>) -> anyhow::Error
where
    CE: std::error::Error + Send + Sync + 'static,
    SE: std::error::Error + Send + Sync + 'static,
    VE: std::error::Error + Send + Sync + 'static,
{
    anyhow::Error::new(e)
}

fn sync(force: bool) -> Result<u8> {
    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let vcs = Git2Vcs::new(root.clone());
    if let Some(state) = vcs.unfinished_state_name()? {
        bail!(
            "git repository is in an unfinished {state} state; \
             finish or abort it (e.g. `bypass git {state} --abort`) \
             before running this command"
        );
    }
    let pull = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["pull", "--rebase"])
        .status()
        .context("spawn `git pull --rebase`")?;
    if !pull.success() {
        bail!(
            "`git pull --rebase` failed (exit {}); see `bypass git status` \
             and resolve before re-running `bypass sync`",
            pull.code().unwrap_or(-1)
        );
    }
    // Lazy-install `.gitattributes` on legacy stores that pre-date the
    // auto-write in `Store::init`. Doing this before the audit so the
    // freshly committed file rides along with the upcoming push.
    install_gitattributes_if_missing(&root)?;
    // The driver registration is idempotent — re-running on every sync
    // upgrades stores cloned after `bypass init` ran. Cheap (two `git
    // config` calls), so we don't bother gating it on the attribute
    // diff.
    sync::merge_driver::register_in_git_config(&root)?;
    if !force {
        let issues = audit::audit_for_push(&root)?;
        if !issues.is_empty() {
            for i in &issues {
                eprintln!(
                    "bypass: {}: {} ({})",
                    i.path.display(),
                    i.kind.describe(),
                    i.detail
                );
            }
            bail!(
                "refusing to push {} suspicious file(s); run `bypass audit` to \
                 review, fix locally, or re-run with `--force` to override",
                issues.len()
            );
        }
    }
    let push = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .arg("push")
        .status()
        .context("spawn `git push`")?;
    if !push.success() {
        bail!(
            "`git push` failed (exit {}); check your remote and credentials",
            push.code().unwrap_or(-1)
        );
    }
    eprintln!("synced.");
    notice_paired_peers()?;
    Ok(0)
}

/// Surface paired peers after a git sync. The pack-exchange building
/// blocks (`sync::syncing`) are in place, but driving them in this
/// one-shot CLI invocation requires either a stored multiaddr per peer
/// or mDNS-driven discovery against a peer that's also listening right
/// now — both arrive with the daemon in Phase 5.2.c. Until then we
/// just remind the user that pairs exist.
fn notice_paired_peers() -> Result<()> {
    let path = match sync::peers::Peers::default_path() {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    let peers = match sync::peers::Peers::load(&path) {
        Ok(p) => p,
        Err(_) => return Ok(()),
    };
    if peers.is_empty() {
        return Ok(());
    }
    eprintln!(
        "bypass: {} paired peer(s) on file; peer-to-peer sync activates \
         when the daemon lands (Phase 5.2.c):",
        peers.records().len()
    );
    for r in peers.records() {
        eprintln!("  - {} ({})", r.name, r.peer_id);
    }
    Ok(())
}

/// Ensure `.gitattributes` carries the `*.gpg binary` rule; if not, write
/// and commit it before continuing. Used by `bypass sync` so legacy
/// stores (created before this rule shipped) get upgraded transparently.
fn install_gitattributes_if_missing(root: &std::path::Path) -> Result<()> {
    let storage = StorageFs::new(root.to_path_buf());
    let crypto = GpgCli::new();
    let vcs = Git2Vcs::new(root.to_path_buf());
    let mut store = Store::new(crypto, storage, vcs);
    let changed = store.install_gitattributes().map_err(map_store_err)?;
    if changed {
        let attrs_path = RelPath::new(".gitattributes").expect(".gitattributes is a valid RelPath");
        // Commit the new file so the rule travels with the push.
        let mut vcs2 = Git2Vcs::new(root.to_path_buf());
        bypass_core::vcs::VersionControl::commit(
            &mut vcs2,
            &[attrs_path],
            "bypass: install .gitattributes for binary `.gpg` files",
        )
        .map_err(anyhow::Error::new)?;
        eprintln!("bypass: installed missing `.gitattributes` rule");
    }
    Ok(())
}

fn sync_identity_rotate(confirm: bool) -> Result<u8> {
    sync::identity::ensure_rotate_confirmed(confirm)?;
    let path = sync::identity::default_path()?;
    let kp = sync::identity::rotate(&path)?;
    // Clearing peers.toml is mandatory per ADR-0015 §Decision step 4:
    // every paired peer was pinning the *old* key.
    let peers_path = sync::peers::Peers::default_path()?;
    let mut peers = sync::peers::Peers::load(&peers_path)?;
    let cleared = peers.records().len();
    peers.clear();
    peers.save(&peers_path)?;
    let new_pid = sync::identity::peer_id(&kp).to_base58();
    eprintln!(
        "Rotated identity. New peer id: {new_pid}\n\
         Cleared {cleared} paired peer(s); re-pair every device with `bypass sync pair`."
    );
    Ok(0)
}

fn sync_pair(show: bool, enter: bool, name: Option<String>, addr: Option<String>) -> Result<u8> {
    if !(show ^ enter) {
        bail!("specify exactly one of --show or --enter");
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async {
        let identity_path = sync::identity::default_path()?;
        let kp = sync::identity::load_or_generate(&identity_path)?;
        let local_name = name.unwrap_or_else(default_device_name);
        if show {
            run_pair_show(kp, addr, local_name).await
        } else {
            let addr = addr.context(
                "--enter requires --addr <multiaddr>; ask the other device to share the \
                 multiaddr printed by `bypass sync pair --show`",
            )?;
            run_pair_enter(kp, addr, local_name).await
        }
    })
}

fn default_device_name() -> String {
    // Try $HOSTNAME first (set on most shells); fall back to a generic
    // label so pairing doesn't fail just because the env var is unset.
    std::env::var("HOSTNAME").unwrap_or_else(|_| "bypass-device".to_string())
}

async fn run_pair_show(
    kp: libp2p_identity::Keypair,
    addr: Option<String>,
    name: String,
) -> Result<u8> {
    use libp2p::multiaddr::Protocol;
    use std::str::FromStr;

    let listen: libp2p::Multiaddr = match addr.as_deref() {
        Some(s) => libp2p::Multiaddr::from_str(s)
            .with_context(|| format!("parse --addr {s:?} as multiaddr"))?,
        None => "/ip4/0.0.0.0/tcp/0".parse().expect("hard-coded multiaddr"),
    };
    let transport =
        sync::libp2p_transport::Libp2pTransport::new(kp.clone(), vec![listen], true).await?;
    let peer_id = transport.local_peer_id();
    // Wait for at least one listen addr to register.
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if !transport.listen_addrs().is_empty() {
            break;
        }
    }
    let listen_addrs = transport.listen_addrs();
    let pin = sync::pairing::generate_pin();
    println!("PAIRING PIN: {pin}");
    println!("Multiaddrs to share with the other device:");
    for a in &listen_addrs {
        let mut full = a.clone();
        full.push(Protocol::P2p(peer_id));
        println!("  {full}");
    }
    eprintln!("waiting for the other device…");
    let paired = sync::pairing::run_show_side(&transport, &pin, &kp, name)
        .await
        .map_err(anyhow::Error::new)?;
    let exit = persist_paired(paired)?;
    // Give the swarm task a moment to flush the final IdentityAck on
    // the wire before we drop the runtime. Without this, the response
    // sits in libp2p's internal queue when the show-side process
    // exits, and the enter-side sees a closed connection on its last
    // request.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    Ok(exit)
}

async fn run_pair_enter(
    kp: libp2p_identity::Keypair,
    addr_str: String,
    name: String,
) -> Result<u8> {
    use libp2p::multiaddr::Protocol;
    use std::str::FromStr;

    let full_addr = libp2p::Multiaddr::from_str(&addr_str)
        .with_context(|| format!("parse --addr {addr_str:?} as multiaddr"))?;
    let peer_id = full_addr
        .iter()
        .find_map(|p| match p {
            Protocol::P2p(pid) => Some(pid),
            _ => None,
        })
        .context(
            "--addr multiaddr must end with /p2p/<peer-id> — copy the full address printed \
             by the other device's `bypass sync pair --show`",
        )?;
    let listen: libp2p::Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().expect("hard-coded multiaddr");
    let transport =
        sync::libp2p_transport::Libp2pTransport::new(kp.clone(), vec![listen], true).await?;
    transport.dial(peer_id, full_addr).await?;
    // Give the dial → Noise → substream handshake time to settle.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let pin = prompt_for_pin()?;
    sync::pairing::validate_pin(&pin).map_err(anyhow::Error::new)?;
    let paired = sync::pairing::run_enter_side(&transport, &peer_id, &pin, &kp, name)
        .await
        .map_err(anyhow::Error::new)?;
    let exit = persist_paired(paired)?;
    // Same flush window as the show-side — gives libp2p's request-
    // response a moment to finish its bookkeeping before the runtime
    // tears the swarm task down.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    Ok(exit)
}

fn prompt_for_pin() -> Result<String> {
    use std::io::Write;
    eprint!("Enter PIN from other device: ");
    std::io::stderr().flush().ok();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).context("read PIN")?;
    Ok(buf.trim().to_owned())
}

fn persist_paired(paired: sync::pairing::PairedPeer) -> Result<u8> {
    let peers_path = sync::peers::Peers::default_path()?;
    let mut peers = sync::peers::Peers::load(&peers_path)?;
    peers.upsert(paired.record.clone());
    peers.save(&peers_path)?;
    eprintln!(
        "paired with {} ({})",
        paired.remote.name, paired.remote.peer_id
    );
    Ok(0)
}

/// Route `bypass sync daemon [op]` to either the foreground daemon
/// (no op) or one of the supervisor ops from ADR-0020.
fn sync_daemon_dispatch(action: Option<cli::SyncDaemonCmd>) -> Result<u8> {
    use cli::SyncDaemonCmd::*;
    match action {
        None => sync_daemon(),
        Some(Install) => sync::service::install(),
        Some(Uninstall) => sync::service::uninstall(),
        Some(Start) => sync::service::start(),
        Some(Stop) => sync::service::stop(),
        Some(Enable) => sync::service::enable(),
        Some(Disable) => sync::service::disable(),
        Some(Status) => sync::service::status(),
    }
}

fn sync_daemon() -> Result<u8> {
    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let identity_path = sync::identity::default_path()?;
    let kp = sync::identity::load_or_generate(&identity_path)?;
    let peers_path = sync::peers::Peers::default_path()?;
    let peers = sync::peers::Peers::load(&peers_path)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async move {
        let listen: libp2p::Multiaddr = "/ip4/0.0.0.0/tcp/0".parse().expect("hard-coded multiaddr");
        let transport = sync::libp2p_transport::Libp2pTransport::new(
            kp.clone(),
            vec![listen],
            /* with_mdns = */ true,
        )
        .await?;
        // Wait briefly for the listen addr to land so the first
        // status snapshot is informative.
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            if !transport.listen_addrs().is_empty() {
                break;
            }
        }

        let watcher = sync::watcher::watch(&root)?;

        let sock_path = sync::socket::default_socket_path()?;
        let listener = sync::socket::bind_or_refuse_existing(&sock_path).await?;
        eprintln!("bypass-sync: status socket at {}", sock_path.display());

        let result = sync::daemon::run(root, transport, peers, peers_path, watcher, listener).await;

        // Best-effort socket cleanup on graceful shutdown.
        let _ = std::fs::remove_file(&sock_path);
        result
    })?;
    Ok(0)
}

fn sync_status(json: bool) -> Result<u8> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;
    rt.block_on(async {
        let path = sync::socket::default_socket_path()?;
        let snap = sync::socket::query_status(&path).await?;
        if json {
            println!(
                "{}",
                serde_json::to_string(&snap).context("encode status as JSON")?
            );
        } else {
            print_status_table(&snap);
        }
        Ok(0)
    })
}

fn print_status_table(snap: &sync::socket::StatusSnapshot) {
    println!("Daemon:    {}", snap.local_peer_id);
    if snap.listening_addrs.is_empty() {
        println!("Listening: (none yet)");
    } else {
        println!("Listening: {}", snap.listening_addrs.join(", "));
    }
    if snap.peers.is_empty() {
        println!("Peers:     (none paired)");
        return;
    }
    println!("Peers:");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    for p in &snap.peers {
        let last = match (&p.last_sync_action, p.last_sync_unix) {
            (Some(action), Some(t)) => format!("{action} ({} ago)", format_relative(now, t)),
            _ => "(never)".to_owned(),
        };
        let disc = if p.discovered { "yes" } else { "no" };
        println!(
            "  {:<12} {:<52}  discovered={disc:<4}  last={last}",
            p.name, p.peer_id
        );
    }
}

fn format_relative(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn sync_peer_rm(name: &str, yes: bool) -> Result<u8> {
    let peers_path = sync::peers::Peers::default_path()?;
    let mut peers = sync::peers::Peers::load(&peers_path)?;
    let Some(record) = peers.find_by_name(name).cloned() else {
        bail!("no such peer: {name:?}");
    };
    let warning = format!(
        "Removing pinning for {name:?} ({pid}).\n\
         Future syncs with this peer will be refused.\n\n\
         Note: prior commits authored by this peer remain in your\n\
         git history. `bypass` does not sign commits per ADR-0014, so\n\
         we cannot reliably distinguish them after the fact. If you\n\
         need a clean history, re-clone from a trusted source or use\n\
         `git filter-repo` to rewrite.",
        pid = record.peer_id
    );
    if !yes {
        eprintln!("{warning}\n\nRe-run with --yes to confirm.");
        return Ok(2);
    }
    let removed = peers
        .remove(name)
        .expect("find_by_name said it exists; remove must succeed");
    peers.save(&peers_path)?;
    eprintln!("{warning}");
    eprintln!("\nremoved {} ({})", removed.name, removed.peer_id);
    Ok(0)
}

fn audit_cmd() -> Result<u8> {
    let root = StorageFs::resolve_default_root().context("resolve store root")?;
    let issues = audit::audit_for_push(&root)?;
    if issues.is_empty() {
        eprintln!("audit: store looks clean");
        Ok(0)
    } else {
        for i in &issues {
            println!("{}: {} ({})", i.path.display(), i.kind.describe(), i.detail);
        }
        Ok(1)
    }
}
