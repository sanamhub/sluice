//! `sluice mcp`: a Model Context Protocol server over standard input and output, in this binary. Each
//! tool calls the same service the daemon serves, in process: there is no socket and no listener,
//! and the server ends when its input closes.

mod tools;
mod types;

use std::path::PathBuf;

use clap::Args;
use rmcp::ServiceExt;
use sluice_api::{Roots, STOP_WAIT, Service};
use sluice_core::engine::{N0_WARNING, RelayModeChoice};
use sluice_core::{Error, Home};

use crate::output::Output;
use tools::Tools;

/// Serves the tools of Sluice to an MCP client over standard input and output.
#[derive(Args)]
pub struct McpArgs {
    /// A folder the tools may send from and receive into, written as a tool names it; repeat for
    /// more. Without one every send and receive is refused
    #[arg(long, value_name = "DIR")]
    pub root: Vec<PathBuf>,
    /// The relay that carries a transfer when a direct path fails
    #[arg(long, value_name = "URL")]
    pub relay: Option<String>,
    /// Use n0's public relays and DNS discovery (not run by this project)
    #[arg(long)]
    pub n0_relays: bool,
    /// Use 127.0.0.1 only, with no relay, no mDNS, no port mapping and no address publishing. It
    /// cannot be combined with `--relay` or `--n0-relays`
    #[arg(long, conflicts_with_all = ["relay", "n0_relays"])]
    pub loopback: bool,
}

/// The longest line of input the server takes.
const MAX_LINE_BYTES: usize = 256 * 1024;

pub fn run(args: &McpArgs, output: &Output) -> Result<u8, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the runtime: {e}")))?;
    runtime.block_on(serve(args, output))
}

async fn serve(args: &McpArgs, output: &Output) -> Result<u8, Error> {
    let relay = if args.loopback {
        RelayModeChoice::Loopback
    } else {
        RelayModeChoice::from_flags(args.relay.as_deref(), args.n0_relays)?
    };
    if args.n0_relays {
        output.write_stderr(&format!("{N0_WARNING}\n"));
    }
    let home = Home::open(&Home::locate()?)?;
    let roots = Roots::open(&args.root).map_err(crate::daemon::refused)?;
    roots
        .refuse_home(home.path())
        .map_err(crate::daemon::refused)?;
    let names = args.root.iter().map(|r| r.display().to_string()).collect();
    let service = Service::new(home, roots, relay);
    // A line of input is a request, and a request is a few paths or a ticket, so a line past a quarter
    // of a megabyte is dropped and not buffered.
    let (input, out) = rmcp::transport::stdio();
    let transport = rmcp::transport::async_rw::AsyncRwTransport::new_server(input, out)
        .with_max_line_length(MAX_LINE_BYTES);
    let running = Tools::new(service.clone(), names)
        .serve(transport)
        .await
        .map_err(|e| Error::Network(format!("cannot start the MCP server: {e}")))?;
    // Ends when the client closes its end. Jobs that still run end as cancelled, with the stores of
    // receives kept, as they do when a daemon stops.
    let ended = running.waiting().await;
    service.stop_jobs(STOP_WAIT).await;
    ended.map_err(|e| Error::Network(format!("the MCP session ended badly: {e}")))?;
    Ok(0)
}
