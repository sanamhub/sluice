//! `sluice send <file>`: import in place, print the ticket, then serve until one download.

use std::{fs, path::PathBuf};

use clap::Args;
use sluice_core::{
    Error, Home,
    engine::{
        N0_WARNING, Outgoing, RelayModeChoice, StopEnd, deadline, now_ms, parse_expiry,
        prepare_send, prepare_send_paths,
    },
};
use tokio::sync::mpsc::unbounded_channel;

use crate::{cli::CongestionArg, events, output::Output};

/// Sends files and folders to whoever has the ticket.
#[derive(Args)]
pub struct SendArgs {
    /// The files and folders to send, or `-` to send standard input as a raw blob (read to the
    /// end and copied into the store, because its root is known only then)
    #[arg(value_name = "PATH", required = true)]
    pub paths: Vec<PathBuf>,
    /// The relay that carries the transfer when a direct path fails
    #[arg(long, value_name = "URL")]
    pub relay: Option<String>,
    /// Use n0's public relays and DNS discovery (not run by this project)
    #[arg(long)]
    pub n0_relays: bool,
    /// How long the ticket works: 1h, 7d, or never
    #[arg(long, default_value = "24h", value_name = "WHEN")]
    pub expires: String,
    /// Keep serving until Ctrl+C instead of stopping after one download. Without this, a sender
    /// stops once every byte is delivered and the receiver has closed its connection, or 30
    /// seconds after the last byte if the receiver keeps the connection open
    #[arg(long)]
    pub keep: bool,
    /// The UDP port to listen on, so a firewall rule can name it. Without it the port this home
    /// used last is tried first
    #[arg(long, value_name = "PORT")]
    pub port: Option<u16>,
    /// The congestion controller: cubic (default) or bbr3 (experimental)
    #[arg(long, value_enum, default_value_t = CongestionArg::Cubic, value_name = "NAME")]
    pub congestion: CongestionArg,
    /// Follow one file that is still being written, sending each segment as it is sealed, instead
    /// of sending what is there now
    #[arg(long)]
    pub live: bool,
    /// For --live, close the stream after the file has not grown for this many seconds
    #[arg(long, default_value_t = 2, value_name = "SECONDS")]
    pub close_after: u64,
}

pub fn run(args: &SendArgs, output: &Output) -> Result<u8, Error> {
    let relay = RelayModeChoice::from_flags(args.relay.as_deref(), args.n0_relays)?;
    if args.n0_relays {
        output.write_stderr(&format!("{N0_WARNING}\n"));
    }
    let seconds = parse_expiry(&args.expires)?;
    let expiry = deadline(now_ms(), seconds);
    let stdin = args.paths.len() == 1 && args.paths[0] == std::path::Path::new("-");
    if !stdin
        && args
            .paths
            .iter()
            .any(|path| path == std::path::Path::new("-"))
    {
        return Err(Error::Policy(
            "standard input is one source, so send it on its own".to_string(),
        ));
    }
    if args.live && (stdin || args.paths.len() != 1) {
        return Err(Error::Policy(
            "a live send follows one regular file, so name exactly one path and no -".to_string(),
        ));
    }
    let paths = if stdin {
        Vec::new()
    } else {
        absolute_paths(&args.paths)?
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the transfer runtime: {e}")))?;
    runtime.block_on(async {
        let home = Home::open(&Home::locate()?)?;
        let outgoing = if args.live {
            sluice_core::engine::prepare_live(
                &home,
                &paths[0],
                relay,
                args.congestion.into(),
                expiry,
                args.port,
                std::time::Duration::from_secs(args.close_after),
            )
            .await?
        } else if stdin {
            sluice_core::engine::prepare_send_stdin(
                &home,
                relay,
                args.congestion.into(),
                expiry,
                args.port,
            )
            .await?
        } else {
            outgoing(&home, &paths, args, relay, expiry).await?
        };
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

/// Every path as an absolute one, made once: the store's import and export refuse a relative path,
/// and a path that is resolved here means the same file whatever the current folder does later.
fn absolute_paths(paths: &[PathBuf]) -> Result<Vec<PathBuf>, Error> {
    paths
        .iter()
        .map(|path| {
            std::path::absolute(path)
                .map_err(|e| Error::Policy(format!("cannot resolve {}: {e}", path.display())))
        })
        .collect()
}

/// One regular file keeps the single-file shape; anything else is a tree of several arguments.
async fn outgoing(
    home: &Home,
    paths: &[PathBuf],
    args: &SendArgs,
    relay: RelayModeChoice,
    expiry: Option<u64>,
) -> Result<Outgoing, Error> {
    let one_file = paths.len() == 1 && fs::metadata(&paths[0]).is_ok_and(|meta| meta.is_file());
    if one_file {
        prepare_send(
            home,
            &paths[0],
            relay,
            args.congestion.into(),
            expiry,
            args.port,
        )
        .await
    } else {
        prepare_send_paths(
            home,
            paths,
            relay,
            args.congestion.into(),
            expiry,
            args.port,
        )
        .await
    }
}
