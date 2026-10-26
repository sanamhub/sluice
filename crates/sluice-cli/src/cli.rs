//! The command line as clap sees it. The binary, the completion scripts and the man pages are all
//! made from this one definition.

use clap::{Parser, Subcommand, ValueEnum};
use sluice_core::engine::Congestion;

use crate::colour::ColorWhen;
use crate::completions;
use crate::doctor;
use crate::id;
use crate::output::Format;
use crate::{recv, send};

/// The congestion controller as the command line spells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum CongestionArg {
    /// Cubic, the default.
    #[default]
    Cubic,
    /// BBRv3, experimental.
    Bbr3,
}

impl From<CongestionArg> for Congestion {
    fn from(value: CongestionArg) -> Self {
        match value {
            CongestionArg::Cubic => Self::Cubic,
            CongestionArg::Bbr3 => Self::Bbr3,
        }
    }
}

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
    /// Send files and folders, print a ticket, and serve until one complete download
    Send(send::SendArgs),
    /// Receive what a ticket names, writing it under the current folder by default
    Recv(recv::RecvArgs),
    /// Check the home, who can reach it, its lock and identity key, and say how to fix a problem
    ///
    /// It only reads: it never creates, locks, repairs or deletes anything. The exit code is 0
    /// unless a check failed, and then that of the most serious failure: 6 for policy, 5 for
    /// storage. Warnings do not fail.
    Doctor(doctor::Args),
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Command, CongestionArg};

    #[test]
    fn the_congestion_flag_parses_and_defaults_to_cubic() {
        let cli = Cli::try_parse_from(["sluice", "send", "a.bin"]).unwrap();
        let Command::Send(args) = cli.command else {
            panic!("send");
        };
        assert_eq!(args.congestion, CongestionArg::Cubic);

        let cli =
            Cli::try_parse_from(["sluice", "recv", "--congestion", "bbr3", "sluice1abc"]).unwrap();
        let Command::Recv(args) = cli.command else {
            panic!("recv");
        };
        assert_eq!(args.congestion, CongestionArg::Bbr3);
    }

    #[test]
    fn an_unknown_controller_is_refused() {
        assert!(Cli::try_parse_from(["sluice", "send", "--congestion", "vegas", "a.bin"]).is_err());
    }
}
