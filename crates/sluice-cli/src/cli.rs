//! The command line as clap sees it. The binary, the completion scripts and the man pages are all
//! made from this one definition.

use clap::{Parser, Subcommand, ValueEnum};
use sluice_core::engine::Congestion;

use crate::colour::ColorWhen;
use crate::completions;
use crate::daemon;
use crate::deliver;
use crate::doctor;
use crate::id;
use crate::inspect;
use crate::mcp;
use crate::output::Format;
use crate::page;
use crate::pair;
use crate::{receipt, recv, send};

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

/// Where a ticket starts among several, as the command line spells it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum PriorityArg {
    /// Starts before every normal and low ticket.
    High,
    /// Starts after a high one and before a low one.
    #[default]
    Normal,
    /// Starts when no high or normal ticket is waiting.
    Low,
}

impl From<PriorityArg> for sluice_core::queue::Priority {
    fn from(value: PriorityArg) -> Self {
        match value {
            PriorityArg::High => Self::High,
            PriorityArg::Normal => Self::Normal,
            PriorityArg::Low => Self::Low,
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
    ///
    /// A received file always keeps owner read and write, whatever the sender recorded: a file the
    /// sender marked read-only arrives writable, and one it marked unreadable arrives readable.
    Recv(recv::RecvArgs),
    /// Read a ticket's manifest and print what it offers, without downloading any content
    ///
    /// It lists the names, how many files and folders there are and how large the transfer is, and
    /// it writes nothing: the manifest is read into a temporary store that is removed again. A live
    /// ticket has no manifest, so it says so.
    Inspect(inspect::InspectArgs),
    /// Check a receipt: the sender's signed statement that it offered some content
    Receipt(receipt::ReceiptArgs),
    /// Check the home, who can reach it, its lock and identity key, and say how to fix a problem
    ///
    /// It only reads: it never creates, locks, repairs or deletes anything. The exit code is 0
    /// unless a check failed, and then that of the most serious failure: 6 for policy, 5 for
    /// storage. Warnings do not fail.
    Doctor(doctor::Args),
    /// Serve the control interface on a socket or pipe only this user can reach, or ask the one
    /// that runs
    ///
    /// The daemon runs transfers for local programs. It sends from and receives into the folders
    /// named with `--root` and nowhere else, and with none it refuses every send and receive. It
    /// holds the home's lock, so a command that opens the same home, `send` among them, is refused
    /// while it runs. `status` and `stop` call a daemon that runs.
    Daemon(daemon::DaemonArgs),
    /// Pair a device with a running daemon, redeem a ticket, or list the daemons this device
    /// paired with
    ///
    /// Without `--redeem` or `--list`, the daemon makes one ticket for one device and one grant.
    /// The ticket is a secret: give it to the device, which redeems it once before it expires.
    /// `--redeem` runs on that device, and keeps the daemon under a name here.
    Pair(pair::PairArgs),
    /// List, rename or revoke the devices a running daemon has granted
    Devices(pair::DevicesArgs),
    /// Hand a file or folder to a daemon this device paired with, which receives it into its inbox
    Deliver(deliver::DeliverArgs),
    /// Serve the tools of Sluice to an MCP client over standard input and output
    ///
    /// The tools send, receive, inspect, list jobs, report a job, cancel and run the doctor. They
    /// work inside the folders named with `--root` and nowhere else, and with none they refuse
    /// every send and receive. A send returns a ticket, which is a secret and which the client's
    /// transcript then holds. The server ends when its input closes.
    Mcp(mcp::McpArgs),
    /// Open the page of the daemon that is running
    ///
    /// The page sends, receives and shows transfers in a browser, and it drives the daemon: it never
    /// carries file bytes. Start the daemon with `sluice daemon --ui`. The address holds a key, which
    /// the browser takes out of it, and it is printed only with `--print`.
    Ui(page::UiArgs),
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

    /// `--qr` takes no value of its own, so the path after it is still the thing to send, and the
    /// two spellings of the flag that name a value keep working.
    #[test]
    fn a_path_before_or_after_the_qr_flag_is_the_path() {
        use crate::qr::Ask;
        use std::path::PathBuf;

        for (args, qr) in [
            (
                vec!["sluice", "send", "--qr", "report.pdf"],
                Some(Ask::Auto),
            ),
            (
                vec!["sluice", "send", "report.pdf", "--qr"],
                Some(Ask::Auto),
            ),
            (
                vec!["sluice", "send", "--qr=inverted", "report.pdf"],
                Some(Ask::Inverted),
            ),
            (vec!["sluice", "send", "report.pdf"], None),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let Command::Send(send) = cli.command else {
                panic!("send");
            };
            assert_eq!(send.paths, [PathBuf::from("report.pdf")]);
            assert_eq!(send.qr, qr);
        }
    }

    #[test]
    fn an_unknown_controller_is_refused() {
        assert!(Cli::try_parse_from(["sluice", "send", "--congestion", "vegas", "a.bin"]).is_err());
    }

    /// A pairing ticket and the list of paired daemons are two different questions, so one command
    /// asks one of them.
    #[test]
    fn pair_takes_a_ticket_or_a_list_and_no_other_grant_is_spelled() {
        use super::Cli as Whole;
        use crate::pair::GrantArg;

        assert!(
            Whole::try_parse_from(["sluice", "pair", "--list", "--redeem", "sluice1abc"]).is_err()
        );
        let cli =
            Whole::try_parse_from(["sluice", "pair", "--grant", "control", "--ttl", "60"]).unwrap();
        let Command::Pair(args) = cli.command else {
            panic!("pair");
        };
        assert_eq!(args.grant, GrantArg::Control);
        assert_eq!(args.ttl, Some(60));
        assert!(Whole::try_parse_from(["sluice", "pair", "--grant", "admin"]).is_err());
    }
}
