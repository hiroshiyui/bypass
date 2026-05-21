// SPDX-License-Identifier: GPL-3.0-or-later

mod cli;
mod clipboard;
mod crypto_gpg;
mod doctor;
mod edit;
mod storage_fs;
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
            let mut store = open_store()?;
            let keys: Vec<KeyId> = gpg_ids.into_iter().map(KeyId::new).collect();
            store.init(&keys).map_err(map_store_err)?;
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
                .insert(&entry, &plaintext, force)
                .map_err(map_store_err)?;
            Ok(0)
        }
        Command::Show { path, clip } => {
            let entry = parse_entry(&path)?;
            let store = open_store()?;
            let plaintext = store.show(&entry).map_err(map_store_err)?;
            if clip {
                let body = plaintext.as_slice();
                // pass's `-c` copies only the first line so login fields
                // below it (line 2+) aren't pasted alongside the password.
                let first_line = body
                    .iter()
                    .position(|&b| b == b'\n')
                    .map(|i| &body[..i])
                    .unwrap_or(body);
                clipboard::copy_and_auto_clear(first_line, DEFAULT_CLEAR_SECS)?;
            } else {
                io::stdout()
                    .write_all(plaintext.as_slice())
                    .context("write stdout")?;
                // Pass appends a trailing newline only if the entry didn't
                // have one; we follow the same rule.
                if plaintext.as_slice().last() != Some(&b'\n') {
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
            let password = generate::generate(length, !no_symbols);
            let mut store = open_store()?;
            if in_place {
                // Replace only the first line; keep the rest of the body.
                let existing = match store.show(&entry) {
                    Ok(b) => b.as_slice().to_vec(),
                    Err(e) => return Err(map_store_err(e)),
                };
                let tail: &[u8] = match existing.iter().position(|&b| b == b'\n') {
                    Some(i) => &existing[i..],
                    None => b"",
                };
                let mut new_body = password.clone().into_bytes();
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
                println!("{password}");
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
        Command::ClipboardSet { seconds } => {
            clipboard::run_daemon(seconds)?;
            Ok(0)
        }
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

fn read_secret_from_stdin(multiline: bool) -> Result<Vec<u8>> {
    if multiline {
        let mut buf = Vec::new();
        io::stdin().read_to_end(&mut buf).context("read stdin")?;
        Ok(buf)
    } else {
        // For non-interactive callers (pipes), read a single line as-is.
        // For interactive callers, prompt twice with echo off.
        if !atty_stdin() {
            let mut line = String::new();
            io::stdin().read_line(&mut line).context("read stdin")?;
            // Strip the trailing newline that read_line includes, mirroring
            // how a user pressing Enter on a TTY would not store the newline.
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(line.into_bytes())
        } else {
            let a = rpassword::prompt_password("Enter password: ").context("read password")?;
            let b = rpassword::prompt_password("Retype password: ").context("confirm password")?;
            if a != b {
                bail!("passwords do not match");
            }
            Ok(a.into_bytes())
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
