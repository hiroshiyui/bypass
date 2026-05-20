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

    /// Check the environment: gpg, keyring, store, recipients, $EDITOR, git.
    Doctor,
}
