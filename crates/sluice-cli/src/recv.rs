//! `sluice recv <ticket>`: decode, fetch, verify, write, and never ask.

use std::{io::BufRead, path::PathBuf};

use clap::Args;
use sluice_core::{
    Error,
    engine::{ReceiveOptions, receive},
    manifest::Limits,
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{cli::CongestionArg, events, output::Output};

/// Receives the file a ticket names.
#[derive(Args)]
pub struct RecvArgs {
    /// The ticket the sender printed, or `-` to read it from standard input
    #[arg(value_name = "TICKET")]
    pub ticket: String,
    /// The folder to write into
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub out: PathBuf,
    /// Replace a path that is already there
    #[arg(long)]
    pub force: bool,
    /// Do not restore the modification time
    #[arg(long)]
    pub no_times: bool,
    /// Create the symlinks the sender recorded (never followed outside the output)
    #[arg(long)]
    pub links: bool,
    /// Accept a top-level name that starts with a dot, such as `.ssh`
    #[arg(long)]
    pub allow_hidden: bool,
    /// The congestion controller: cubic (default) or bbr3 (experimental)
    #[arg(long, value_enum, default_value_t = CongestionArg::Cubic, value_name = "NAME")]
    pub congestion: CongestionArg,
}

pub fn run(args: &RecvArgs, output: &Output) -> Result<u8, Error> {
    let ticket = ticket_text(&args.ticket)?;
    let out = std::path::absolute(&args.out)
        .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", args.out.display())))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, rx) = unbounded_channel();
        let printer = events::printer(*output, rx, "receiving");
        let options = ReceiveOptions {
            out,
            force: args.force,
            no_times: args.no_times,
            links: args.links,
            allow_hidden: args.allow_hidden,
            limits: Limits::default(),
            free_bytes: None,
            congestion: args.congestion.into(),
            max_network_failures: sluice_core::queue::MAX_NETWORK_FAILURES,
        };
        let result = receive(&ticket, &options, &tx).await;
        drop(tx);
        let printed = printer
            .await
            .map_err(|e| Error::Bug(format!("the event printer stopped: {e}")))?;
        printed?;
        let received = result?;
        if matches!(output.format(), crate::output::Format::Human) {
            if received.files == 1 {
                output.write_stderr(&format!("wrote {}\n", received.path.display()));
            } else {
                output.write_stderr(&format!(
                    "wrote {} files under {}\n",
                    received.files,
                    received.path.display()
                ));
            }
        }
        Ok(0)
    })
}

/// The ticket as given, or the one line on standard input when the argument is `-`. A ticket on
/// the command line is visible in the process list and the shell history, so a caller that cares
/// pipes it instead.
fn ticket_text(argument: &str) -> Result<String, Error> {
    if argument != "-" {
        return Ok(argument.to_string());
    }
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| Error::Storage(format!("cannot read the ticket from standard input: {e}")))?;
    let text = line.trim().to_string();
    if text.is_empty() {
        return Err(Error::Policy(
            "no ticket arrived on standard input".to_string(),
        ));
    }
    Ok(text)
}
