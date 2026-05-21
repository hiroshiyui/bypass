// SPDX-License-Identifier: GPL-3.0-or-later

//! Command-line interface definition. Subcommand handlers are implemented
//! in later milestones; this module only declares the surface.

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "bypass",
    version,
    about = "A pass-compatible password manager.",
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new password store for the given GPG recipient(s).
    Init {
        /// GPG key id(s) that can decrypt entries in this store.
        #[arg(required = true)]
        gpg_ids: Vec<String>,
    },

    /// Insert a new password entry, reading the secret from stdin.
    Insert {
        /// Entry path, e.g. `email/work`.
        path: String,
        /// Allow overwriting an existing entry.
        #[arg(short, long)]
        force: bool,
        /// Read a multi-line entry until EOF instead of a single line.
        #[arg(short, long)]
        multiline: bool,
    },

    /// Decrypt and print an entry.
    Show {
        /// Entry path.
        path: String,
        /// Optional field name. When given, print (or copy with `-c`)
        /// only the value of that field instead of the whole entry.
        /// Field matching is case-insensitive.
        field: Option<String>,
        /// Copy the chosen output to the system clipboard for ~45
        /// seconds instead of printing it. Without a field this copies
        /// the first line (the password); with a field it copies the
        /// field value. The previous clipboard contents are restored
        /// when the timer elapses.
        #[arg(short = 'c', long = "clip")]
        clip: bool,
    },

    /// List entries as a tree.
    Ls {
        /// Optional subpath to list.
        subpath: Option<String>,
    },

    /// Search entry names matching a pattern.
    Find {
        /// Pattern to match.
        pattern: String,
    },

    /// Remove an entry.
    Rm {
        /// Entry path.
        path: String,
        /// Remove directories recursively.
        #[arg(short, long)]
        recursive: bool,
    },

    /// Decrypt an entry into a tempfile, open `$EDITOR`, then re-encrypt.
    Edit {
        /// Entry path.
        path: String,
    },

    /// Generate a strong random password, store it, and print it.
    Generate {
        /// Entry path.
        path: String,
        /// Password length. Defaults to 25 (matches `pass`).
        length: Option<usize>,
        /// Use the alphanumeric alphabet only (no punctuation).
        #[arg(short, long)]
        no_symbols: bool,
        /// Replace only the first line of an existing entry, keeping the
        /// rest of the body intact. Implies overwrite.
        #[arg(short = 'i', long)]
        in_place: bool,
        /// Overwrite an existing entry. Ignored when `--in-place` is set.
        #[arg(short, long)]
        force: bool,
        /// Copy the generated password to the clipboard instead of
        /// printing it. Auto-clears after ~45 seconds.
        #[arg(short = 'c', long = "clip")]
        clip: bool,
    },

    /// Copy an entry.
    Cp {
        /// Source entry path.
        from: String,
        /// Destination entry path.
        to: String,
        /// Allow overwriting an existing destination.
        #[arg(short, long)]
        force: bool,
    },

    /// Move (rename) an entry.
    Mv {
        /// Source entry path.
        from: String,
        /// Destination entry path.
        to: String,
        /// Allow overwriting an existing destination.
        #[arg(short, long)]
        force: bool,
    },

    /// Show commit history. With a path, only commits that touched the
    /// matching entry (or anything below it) are shown.
    Log {
        /// Optional entry path or subtree prefix.
        path: Option<String>,
    },

    /// Compute the current TOTP code for an entry containing an
    /// `otpauth://` URI.
    Otp {
        /// Entry path.
        path: String,
        /// Copy the code to the clipboard instead of printing it.
        /// Auto-clears after ~45 seconds.
        #[arg(short = 'c', long = "clip")]
        clip: bool,
    },

    /// Check the environment: gpg, keyring, store, recipients, $EDITOR, git.
    Doctor,

    /// Run the system `git` against the store's repository.
    /// Anything after `bypass git` is passed through verbatim.
    Git {
        /// Arguments to forward to `git`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Run a pass-style extension. Searches `<store>/.extensions/<name>`,
    /// `$PASSWORD_STORE_EXTENSIONS_DIR/<name>`, and
    /// `~/.password-store-extensions/<name>` in that order.
    Ext {
        /// Extension name.
        name: String,
        /// Arguments forwarded to the extension.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },

    /// Internal: clipboard auto-clear daemon. Spawned by `show -c` /
    /// `generate -c` via re-exec. Reads the password to install from
    /// stdin. Not meant for direct user invocation.
    #[command(hide = true, name = "__clipboard-set")]
    ClipboardSet {
        /// Seconds to keep the password on the clipboard before
        /// restoring whatever was there before.
        seconds: u64,
    },
}
