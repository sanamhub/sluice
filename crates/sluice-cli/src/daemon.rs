//! `sluice daemon`: the control interface served on a socket or pipe only this user can reach, and
//! the two thin clients `status` and `stop`. `send` and `recv` stay one-shot and never use it.

mod client;

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
    /// A folder the daemon may send from and receive into, written as a client will name it;
    /// repeat for more. Without one the daemon refuses every send and every receive
    #[arg(long, value_name = "DIR")]
    pub root: Vec<PathBuf>,
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
    let daemon = Daemon::bind(home, roots, relay).map_err(refused)?;
    tracing::info!("listening on {}", daemon.location());
    output.write_stdout(&listening_line(output, daemon.location()))?;
    daemon.serve(signalled()).await.map_err(refused)?;
    tracing::info!("stopped");
    Ok(0)
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

/// Resolves on Ctrl+C, and on Unix on a termination signal as well.
async fn signalled() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The kind of error that goes with each kind of refusal, so the exit code says what happened.
fn refused(error: ApiError) -> Error {
    let text = error.to_string();
    match error {
        ApiError::Invalid(_) | ApiError::Refused(_) | ApiError::Busy(_) => Error::Policy(text),
        ApiError::Unavailable(_) => Error::Network(text),
        ApiError::NotFound(_) | ApiError::Failed(_) => Error::Storage(text),
    }
}
