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

    /// Sync the store with its git remote: `git pull --rebase` then
    /// `git push`. Before pushing, runs the same leak check as
    /// `bypass audit` over the commits about to be published.
    ///
    /// With no subcommand: runs the default git-based sync. With a
    /// subcommand: pair another device, manage the local identity
    /// key, etc.
    Sync {
        /// Skip the leak-check audit. Only meaningful when no
        /// subcommand is given (i.e. for the default sync action).
        #[arg(long)]
        force: bool,
        #[command(subcommand)]
        sub: Option<SyncCmd>,
    },

    /// Inspect the local store for files that don't look like OpenPGP
    /// ciphertext or recognised metadata. Scans the unpushed commits
    /// (`@{upstream}..HEAD`), falling back to the full tracked set on
    /// stores without an upstream. Exits 0 when clean, 1 when issues
    /// are found.
    Audit,

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

    /// Internal: custom git merge driver registered via
    /// `.gitattributes` (`merge=bypass-take-theirs`). Always resolves
    /// a `.gpg` conflict by taking the incoming side, since
    /// ciphertext blobs are opaque and have no meaningful 3-way merge
    /// ([ADR-0011](../../doc/adr/0011-sync-semantics-hybrid.md)). Not
    /// meant for direct user invocation.
    #[command(hide = true, name = "__merge-take-theirs")]
    MergeTakeTheirs {
        /// `%O` — ancestor blob path.
        ancestor: String,
        /// `%A` — current (ours) blob path. The driver writes the
        /// resolved content here.
        ours: String,
        /// `%B` — other (theirs) blob path.
        theirs: String,
        /// `%P` — pathname (for diagnostics).
        path: String,
        /// `%L` — conflict-marker-size (unused).
        marker_size: String,
    },
}

/// Sub-actions under `bypass sync`.
#[derive(Debug, Subcommand)]
pub enum SyncCmd {
    /// Pair this device with another via the PAKE-from-PIN flow
    /// (ADR-0012). One side runs `--show` to display a PIN; the other
    /// runs `--enter` and types it in.
    Pair {
        /// Display a PIN and wait for the other device.
        #[arg(long, conflicts_with = "enter")]
        show: bool,
        /// Type the PIN displayed on the other device.
        #[arg(long, conflicts_with = "show")]
        enter: bool,
        /// Friendly name to record for the local device in the paired
        /// peer's `peers.toml`. Defaults to the system hostname.
        #[arg(long)]
        name: Option<String>,
        /// Multiaddr to listen on (`--show`) or to dial (`--enter`).
        /// Show-side defaults to `/ip4/0.0.0.0/tcp/0`. Enter-side
        /// requires the multiaddr printed by the show-side until
        /// mDNS-driven discovery lands in 5.2.c.
        #[arg(long)]
        addr: Option<String>,
    },

    /// Manage this device's libp2p identity key.
    Identity {
        #[command(subcommand)]
        action: SyncIdentityCmd,
    },
}

#[derive(Debug, Subcommand)]
pub enum SyncIdentityCmd {
    /// Generate a fresh Ed25519 identity, overwriting the existing
    /// one and clearing `peers.toml`. Requires `--confirm` because
    /// rotation invalidates every paired peer relationship.
    Rotate {
        /// Acknowledge that rotation destroys all pairings.
        #[arg(long)]
        confirm: bool,
    },
}
