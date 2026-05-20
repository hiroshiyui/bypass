// SPDX-License-Identifier: GPL-3.0-or-later

mod cli;
mod crypto_gpg;

use anyhow::{Result, bail};
use clap::Parser;

use crate::cli::{Cli, Command};

fn main() -> Result<()> {
    let args = Cli::parse();
    match args.command {
        Command::Init { .. } => bail!("`init` is not implemented yet"),
        Command::Insert { .. } => bail!("`insert` is not implemented yet"),
        Command::Show { .. } => bail!("`show` is not implemented yet"),
        Command::Ls { .. } => bail!("`ls` is not implemented yet"),
        Command::Find { .. } => bail!("`find` is not implemented yet"),
        Command::Rm { .. } => bail!("`rm` is not implemented yet"),
        Command::Edit { .. } => bail!("`edit` is not implemented yet"),
        Command::Cp { .. } => bail!("`cp` is not implemented yet"),
        Command::Mv { .. } => bail!("`mv` is not implemented yet"),
    }
}
