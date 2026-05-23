// SPDX-License-Identifier: GPL-3.0-or-later

//! Command-line interface definition. Subcommand handlers are implemented
//! in later milestones; this module only declares the surface.

use clap::{Parser, Subcommand};
pub use clap_complete::Shell;

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
        /// Overwrite an existing `.gpg-id` without re-encrypting
        /// existing entries. The default refuses on an already-
        /// initialised store because changing the recipient leaves
        /// stored blobs encrypted to the OLD key, while new inserts
        /// would target the new key — silently splitting the store
        /// across two recipients. Future entries will fail to encrypt
        /// if the new recipient is missing from the keyring.
        #[arg(short, long)]
        force: bool,
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

    /// Emit a shell completion script for `bypass` to stdout. Redirect
    /// to wherever your shell expects completion files, e.g.
    /// `bypass completion bash > /etc/bash_completion.d/bypass`.
    Completion {
        /// Target shell. One of: `bash`, `zsh`, `fish`, `powershell`,
        /// `elvish`.
        shell: Shell,
    },

    /// Emit the `bypass(1)` man page in groff/troff format to stdout.
    /// Redirect to your `man1/` directory, e.g.
    /// `bypass man > /usr/local/share/man/man1/bypass.1`.
    Man,

    /// Native-messaging host for the browser extension (ADR-0022).
    ///
    /// With no sub-action, runs the host: reads length-prefixed JSON
    /// requests on stdin and writes length-prefixed JSON replies on
    /// stdout until the browser closes the pipe. The browser spawns
    /// this; users normally don't invoke it by hand. With
    /// `install` / `uninstall`, manages the per-browser manifest
    /// files that tell Firefox / Chrome where to find this binary.
    MessagingHost {
        #[command(subcommand)]
        action: Option<MessagingHostCmd>,
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

    /// Run the foreground sync daemon (Phase 5.2.c), or manage the
    /// platform supervisor that auto-runs it on login / after a
    /// crash (Phase 6 — ADR-0020).
    ///
    /// With no sub-action, runs the daemon foreground. With one of
    /// `install` / `uninstall` / `start` / `stop` / `enable` /
    /// `disable` / `status`, drives systemd (Linux) or launchd
    /// (macOS) instead — see `bypass sync daemon <op> --help`.
    Daemon {
        #[command(subcommand)]
        action: Option<SyncDaemonCmd>,
    },

    /// Print a snapshot of the running daemon's view: local peer
    /// id, listening multiaddrs, paired peers and their last
    /// sync action.
    Status {
        /// Emit the raw JSON reply instead of the human table.
        #[arg(long)]
        json: bool,
    },

    /// Manage paired peers.
    Peer {
        #[command(subcommand)]
        action: SyncPeerCmd,
    },
}

/// Sub-actions under `bypass sync peer`.
#[derive(Debug, Subcommand)]
pub enum SyncPeerCmd {
    /// Revoke a paired peer by friendly name. See
    /// [ADR-0019](../../doc/adr/0019-peer-revocation-trust-semantics.md):
    /// future syncs with this peer are refused, but prior commits
    /// in your git history are not rewritten.
    Rm {
        /// The friendly name from pairing.
        name: String,
        /// Acknowledge that revocation does not rewrite history.
        #[arg(long)]
        yes: bool,
    },
}

/// Sub-actions under `bypass sync daemon`. Without any of these,
/// the bare `bypass sync daemon` command runs the daemon in the
/// foreground. See [ADR-0020](../../doc/adr/0020-daemon-service-supervision.md).
#[derive(Debug, Subcommand)]
pub enum SyncDaemonCmd {
    /// Write the systemd user unit (Linux) or launchd plist
    /// (macOS) so the daemon can be supervisor-managed. Does not
    /// start the daemon or enable autostart — those are
    /// explicit follow-up steps. Re-run after upgrading
    /// `bypass` so the supervisor sees the new binary path.
    Install,
    /// Remove the supervisor file written by `install`.
    Uninstall,
    /// Ask the supervisor to start the daemon now (not
    /// boot-persistent unless `enable` was also run).
    Start,
    /// Ask the supervisor to stop the daemon now.
    Stop,
    /// Configure the supervisor to auto-start the daemon on
    /// login.
    Enable,
    /// Stop auto-starting the daemon on login.
    Disable,
    /// Print the supervisor's view of the daemon: is it
    /// running, is it enabled, last exit code. Distinct from
    /// `bypass sync status` (which queries the live daemon's
    /// peer-state snapshot — see ADR-0018).
    Status,
}

/// Sub-actions under `bypass messaging-host`. With no sub, the
/// bare `bypass messaging-host` command runs the host — see
/// [ADR-0023](../../doc/adr/0023-browser-extension-architecture.md).
#[derive(Debug, Subcommand)]
pub enum MessagingHostCmd {
    /// Write the native-messaging manifest at the conventional
    /// per-browser path so Firefox / Chrome can locate this
    /// binary. Always installs the Firefox manifest. Chrome /
    /// Chromium manifests are written only when `--chrome-id` is
    /// supplied (the extension's id is autogenerated when you
    /// load it unpacked — copy it from `chrome://extensions`).
    ///
    /// Re-run after upgrading `bypass` so the manifest path stays
    /// in sync with the actual binary.
    Install {
        /// Chrome / Chromium extension id (32-char string from
        /// `chrome://extensions` after loading unpacked).
        #[arg(long)]
        chrome_id: Option<String>,
        /// Override the default Firefox extension id
        /// (`bypass@bypass.example`).
        #[arg(long)]
        firefox_id: Option<String>,
    },

    /// Remove the native-messaging manifests at every
    /// conventional per-browser path. Best effort: paths that
    /// aren't present are silently skipped.
    Uninstall,
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
