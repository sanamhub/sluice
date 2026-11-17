//! `sluice deliver <device> <path>`: hand a file or folder to a daemon this device paired with,
//! which receives it into its inbox. The delivery connection carries the ticket; the content goes
//! over the static transport from this device to the daemon, and this command serves it.

use std::path::PathBuf;

use clap::Args;
use sluice_core::engine::{self, Congestion, Keep, StopEnd, prepare_send, prepare_send_paths};
use sluice_core::paired::Paired;
use sluice_core::{Error, Home};
use tokio::sync::mpsc::unbounded_channel;

use crate::output::Output;

/// Hands a file or folder to a paired daemon.
#[derive(Args)]
pub struct DeliverArgs {
    /// The paired daemon's name, or a prefix of its endpoint id
    #[arg(value_name = "DEVICE")]
    pub device: String,
    /// The file or folder to deliver
    #[arg(value_name = "PATH")]
    pub path: PathBuf,
    /// The top-level name the content takes under the daemon's inbox. Without it the delivery is
    /// named by the ticket's root in hex
    #[arg(long, value_name = "NAME")]
    pub name: Option<String>,
}

/// Runs `sluice deliver`.
pub fn run(args: &DeliverArgs, output: &Output) -> Result<u8, Error> {
    let home = Home::open(&Home::locate()?)?;
    let daemon = Paired::open(home.path()).find(&args.device)?;
    let relay = match &daemon.relay {
        Some(url) => engine::RelayModeChoice::Url(url.clone()),
        None => engine::RelayModeChoice::Disabled,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the runtime: {e}")))?;
    runtime.block_on(async {
        let mut outgoing = if args.path.is_dir() {
            prepare_send_paths(
                &home,
                std::slice::from_ref(&args.path),
                relay,
                Congestion::Cubic,
                None,
                None,
            )
            .await?
        } else {
            prepare_send(&home, &args.path, relay, Congestion::Cubic, None, None).await?
        };
        outgoing.reserve(Keep::One);
        let text = outgoing.ticket.encode();
        let endpoint = outgoing.provider.endpoint().clone();
        let status = engine::deliver_to(&endpoint, &daemon, &text, args.name.as_deref()).await?;
        if status != 0x00 {
            let _ = outgoing.provider.shutdown().await;
            return Err(match status {
                0x02 => Error::Storage("the daemon could not start the receive".to_string()),
                _ => Error::Policy(format!(
                    "the daemon refused the delivery with status 0x{status:02x}; the ticket is \
                     expired or not for this daemon, the inbox is full, or the name is taken"
                )),
            });
        }
        let (tx, rx) = unbounded_channel();
        let printer = crate::events::printer(*output, rx, "delivering");
        let end = outgoing.serve(Keep::One, &tx).await;
        drop(tx);
        let printed = printer
            .await
            .map_err(|e| Error::Bug(format!("the event printer stopped: {e}")))?;
        printed?;
        match end? {
            StopEnd::Downloaded | StopEnd::Interrupted => Ok(0),
            StopEnd::Expired => Err(Error::Policy(
                "the ticket expired before the daemon took every byte".to_string(),
            )),
        }
    })
}
