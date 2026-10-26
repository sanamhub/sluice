//! The command line as clap sees it. The binary, the completion scripts and the man pages are all
//! made from this one definition.

use clap::{Parser, Subcommand};

use crate::colour::ColorWhen;
use crate::completions;
use crate::doctor;
use crate::id;
use crate::output::Format;
use crate::{recv, send};

#[derive(Parser)]
// `bin_name` too: clap otherwise prints the name of the executable, which is `sluice.exe` on
// Windows.
#[command(name = "sluice", bin_name = "sluice", version)]
pub struct Cli {
    /// Print JSON, one object per line, instead of text. A failure is then a JSON line on stderr
    #[arg(long, global = true)]
    json: bool,
    /// When to colour help, errors and other output. NO_COLOR set to anything turns it off
    #[arg(long, global = true, value_enum, default_value_t = ColorWhen::Auto, value_name = "WHEN")]
    pub color: ColorWhen,
    #[command(subcommand)]
    pub command: Command,
}

impl Cli {
    /// Text unless `--json` was given. Whether stdout is a terminal does not come into it, so
    /// `sluice id | clip` gets the same bytes as `sluice id` on a terminal.
    pub fn format(&self) -> Format {
        if self.json {
            Format::Json
        } else {
            Format::Human
        }
    }
}

#[derive(Subcommand)]
pub enum Command {
    /// Print the endpoint id of this machine's identity, or replace the identity
    Id(id::Args),
    /// Print the completion script for a shell
    Completions(completions::Args),
    /// Send a file, print its ticket, and serve until one complete download
    Send(send::SendArgs),
    /// Receive the file a ticket names, writing it under the current folder by default
    Recv(recv::RecvArgs),
    /// Check the home, who can reach it, its lock and identity key, and say how to fix a problem
    ///
    /// It only reads: it never creates, locks, repairs or deletes anything. The exit code is 0
    /// unless a check failed, and then that of the most serious failure: 6 for policy, 5 for
    /// storage. Warnings do not fail.
    Doctor(doctor::Args),
}
