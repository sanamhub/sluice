//! `sluice recv <ticket>`: decode, fetch, verify, write, and never ask.

use std::path::PathBuf;

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
    /// The ticket the sender printed
    #[arg(value_name = "TICKET")]
    pub ticket: String,
    /// The folder to write into
    #[arg(long, default_value = ".", value_name = "DIR")]
    pub out: PathBuf,
    /// Replace a file that is already there
    #[arg(long)]
    pub force: bool,
    /// Do not restore the modification time
    #[arg(long)]
    pub no_times: bool,
    /// Create the symlinks the sender recorded (never followed outside the output)
    #[arg(long)]
    pub links: bool,
    /// The congestion controller: cubic (default) or bbr3 (experimental)
    #[arg(long, value_enum, default_value_t = CongestionArg::Cubic, value_name = "NAME")]
    pub congestion: CongestionArg,
}

pub fn run(args: &RecvArgs, output: &Output) -> Result<u8, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let (tx, rx) = unbounded_channel();
        let printer = events::printer(*output, rx, "receiving");
        let options = ReceiveOptions {
            out: args.out.clone(),
            force: args.force,
            no_times: args.no_times,
            links: args.links,
            limits: Limits::default(),
            free_bytes: None,
            congestion: args.congestion.into(),
        };
        let result = receive(&args.ticket, &options, &tx).await;
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
