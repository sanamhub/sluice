//! `sluice send <file>`: import in place, print the ticket, then serve until one download.

use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Args;
use sluice_core::{
    Error, Home,
    engine::{N0_WARNING, RelayModeChoice, StopEnd, deadline, parse_expiry, prepare_send},
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{events, output::Output};

/// Sends one file to whoever has the ticket.
#[derive(Args)]
pub struct SendArgs {
    /// The file to send
    #[arg(value_name = "FILE")]
    pub file: PathBuf,
    /// The relay that carries the transfer when a direct path fails
    #[arg(long, value_name = "URL")]
    pub relay: Option<String>,
    /// Use n0's public relays and DNS discovery (not run by this project)
    #[arg(long)]
    pub n0_relays: bool,
    /// How long the ticket works: 1h, 7d, or never
    #[arg(long, default_value = "24h", value_name = "WHEN")]
    pub expires: String,
    /// Keep serving until Ctrl+C instead of stopping after one download
    #[arg(long)]
    pub keep: bool,
}

pub fn run(args: &SendArgs, output: &Output) -> Result<u8, Error> {
    let relay = RelayModeChoice::from_flags(args.relay.as_deref(), args.n0_relays)?;
    if args.n0_relays {
        output.write_stderr(&format!("{N0_WARNING}\n"));
    }
    let seconds = parse_expiry(&args.expires)?;
    let expiry = deadline(now_ms(), seconds);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let home = Home::open(&Home::locate()?)?;
        let outgoing = prepare_send(&home, &args.file, relay, expiry).await?;
        events::ticket(*output, &outgoing.ticket.encode())?;
        let (tx, rx) = unbounded_channel();
        let printer = events::printer(*output, rx, "sending");
        let end = outgoing.serve(args.keep, &tx).await;
        drop(tx);
        let printed = printer
            .await
            .map_err(|e| Error::Bug(format!("the event printer stopped: {e}")))?;
        printed?;
        match end? {
            StopEnd::Downloaded | StopEnd::Interrupted => Ok(0),
            StopEnd::Expired => {
                output.write_stderr("the ticket expired, so this send stopped serving it\n");
                Ok(0)
            }
        }
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}
