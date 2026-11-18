//! `sluice daemon`: the control interface served on a socket or pipe only this user can reach, and
//! the two thin clients `status` and `stop`. `send` and `recv` stay one-shot and never use it.

mod client;
mod signals;

use std::path::PathBuf;

use clap::{Args, Subcommand};
use sluice_api::{ApiError, Daemon, NO_ROOTS, Roots};
use sluice_core::engine::{N0_WARNING, RelayModeChoice};
use sluice_core::{Error, Home};

use crate::output::Output;

/// What the daemon does.
#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: Option<Action>,
    /// A folder the daemon may send from and receive into, written as a client names it;
    /// repeat for more. Without one the daemon refuses every send and every receive
    #[arg(long, value_name = "DIR")]
    pub root: Vec<PathBuf>,
    /// Receive deliveries into this folder, which must be inside a root. Without it the daemon
    /// makes pairing tickets but takes no delivery
    #[arg(long, value_name = "DIR")]
    pub inbox: Option<PathBuf>,
    /// The most content the inbox may hold, in bytes (default 10 GiB)
    #[arg(
        long,
        value_name = "BYTES",
        default_value_t = sluice_core::engine::DEFAULT_INBOX_QUOTA
    )]
    pub inbox_quota: u64,
    /// The relay that carries a transfer when a direct path fails
    #[arg(long, value_name = "URL")]
    pub relay: Option<String>,
    /// Use n0's public relays and DNS discovery (not run by this project)
    #[arg(long)]
    pub n0_relays: bool,
    /// Listen for transfers on 127.0.0.1 only, with no relay, no mDNS, no port mapping and no
    /// address publishing, so nothing outside this computer can reach them. It cannot be combined
    /// with `--relay` or `--n0-relays`
    #[arg(long, conflicts_with_all = ["relay", "n0_relays"])]
    pub loopback: bool,
    /// Also serve a page and the control interface as JSON over HTTP on 127.0.0.1, on a port the
    /// system picks, for `sluice ui`. A key made at start guards every call, and the port and key go
    /// to a file in the home that only this user can read
    #[arg(long)]
    pub ui: bool,
}

/// What a client of a running daemon asks.
#[derive(Subcommand)]
pub enum Action {
    /// Print the running daemon's version and its jobs
    Status,
    /// Stop the running daemon: its jobs end as cancelled, a receive keeps its partial store, and
    /// the socket or pipe is removed
    Stop,
}

pub fn run(args: &DaemonArgs, output: &Output) -> Result<u8, Error> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the daemon's runtime: {e}")))?;
    runtime.block_on(async {
        match args.action {
            None => serve(args, output).await,
            Some(Action::Status) => client::status(output).await,
            Some(Action::Stop) => client::stop(output).await,
        }
    })
}

/// Opens the home, listens and serves until a signal or a `stop`.
async fn serve(args: &DaemonArgs, output: &Output) -> Result<u8, Error> {
    init_logging();
    // Registered before the daemon says where it listens, so a signal that follows is never lost.
    let signals = signals::Signals::register()?;
    let relay = if args.loopback {
        RelayModeChoice::Loopback
    } else {
        RelayModeChoice::from_flags(args.relay.as_deref(), args.n0_relays)?
    };
    if args.n0_relays {
        output.write_stderr(&format!("{N0_WARNING}\n"));
    }
    let home = Home::open(&Home::locate()?)?;
    let roots = Roots::open(&args.root).map_err(refused)?;
    if roots.is_empty() {
        output.write_stderr(&format!("sluice: {NO_ROOTS}\n"));
    }
    let daemon = Daemon::bind(home, roots, relay, args.inbox.clone(), args.inbox_quota)
        .await
        .map_err(refused)?;
    let page = start_page(args, &daemon).await?;
    tracing::info!("listening on {}", daemon.location());
    output.write_stdout(&listening_line(output, daemon.location()))?;
    let served = daemon.serve(signals.arrived()).await;
    if let Some((page, service)) = page {
        page.stop();
        service.remove_private(crate::ui::PORT_FILE);
    }
    served.map_err(refused)?;
    tracing::info!("stopped");
    Ok(0)
}

/// Starts the page's listener when `--ui` was given, and writes its port and key where `sluice ui`
/// reads them. The key is in that file and nowhere else: not on a console and not in a log.
async fn start_page(
    args: &DaemonArgs,
    daemon: &Daemon,
) -> Result<Option<(crate::ui::Page, sluice_api::Service)>, Error> {
    if !args.ui {
        // A port file can be left by a daemon that died before it removed the file. This daemon
        // does not serve a page, so the file is not its own and must not outlive its start: `stop`
        // takes the file's presence as the sign that the daemon it asked had a page.
        daemon.service().remove_private(crate::ui::PORT_FILE);
        return Ok(None);
    }
    let service = daemon.service();
    let page = crate::ui::start(service.clone()).await?;
    service
        .write_private(crate::ui::PORT_FILE, page.file_text().as_bytes())
        .map_err(refused)?;
    tracing::info!("the page is on 127.0.0.1 port {}", page.port());
    Ok(Some((page, service)))
}

/// The one line on standard output that says where the daemon listens, which a script waits for.
fn listening_line(output: &Output, location: &str) -> String {
    match output.format() {
        crate::output::Format::Human => format!("listening on {location}\n"),
        crate::output::Format::Json => {
            format!("{}\n", serde_json::json!({ "listening": location }))
        }
    }
}

/// The daemon's own log lines go to standard error, at `info` unless `RUST_LOG` says otherwise. A
/// ticket is never in one: the service has no way to log it.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // A second call, in a test that runs the command twice in one process, finds one already set.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}

/// The kind of error that goes with each kind of refusal, so the exit code says what happened.
pub(crate) fn refused(error: ApiError) -> Error {
    let text = error.to_string();
    match error {
        ApiError::Invalid(_) | ApiError::Refused(_) | ApiError::Busy(_) => Error::Policy(text),
        ApiError::Unavailable(_) => Error::Network(text),
        ApiError::NotFound(_) | ApiError::Failed(_) => Error::Storage(text),
    }
}
