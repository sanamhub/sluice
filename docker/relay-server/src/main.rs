//! `sluice-relay --config-path <file>`: the relay image's server. The guard has already checked the
//! same file and handed it over on `/proc/self/fd/0`; this binary reads it, builds `iroh-relay`'s
//! relay service with the `/meet` handler beside `/relay`, and serves both on one listener.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use sluice_relay::{config, meet, server};

/// `EX_USAGE`: the config is not one this binary reads.
const EXIT_REFUSED: u8 = 64;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args == [OsString::from("--version")] {
        println!("sluice-relay {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    let path = match config_path(args.into_iter()) {
        Ok(Some(path)) => path,
        Ok(None) => {
            eprintln!("sluice-relay: --config-path <file> is required");
            return ExitCode::from(EXIT_REFUSED);
        }
        Err(error) => {
            eprintln!("sluice-relay: {error}");
            return ExitCode::from(EXIT_REFUSED);
        }
    };
    logs();
    match run(path) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sluice-relay: {error}");
            ExitCode::from(EXIT_REFUSED)
        }
    }
}

fn run(path: PathBuf) -> Result<()> {
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let config = config::Config::parse(&text).map_err(|error| anyhow::anyhow!("{error}"))?;
    let limits = config.meet;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start the runtime")?;
    runtime.block_on(async move {
        let meet = limits.map(meet::MeetHttp::new);
        let running = server::start(config, meet).await?;
        tracing::info!("the relay listens on {}", running.addr);
        stop().await;
        running.shutdown().await
    })
}

/// The one argument the guard passes. `--config-path <file>` and `--config-path=<file>` are the two
/// spellings, as the guard's own parser has them.
fn config_path(args: impl Iterator<Item = OsString>) -> Result<Option<PathBuf>> {
    let mut path = None;
    let mut rest = args;
    while let Some(arg) = rest.next() {
        let value = if arg == "--config-path" {
            rest.next()
        } else if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix("--config-path="))
        {
            Some(OsString::from(value))
        } else {
            bail!("unknown argument");
        };
        let Some(value) = value else {
            bail!("--config-path needs a file");
        };
        if path.replace(PathBuf::from(value)).is_some() {
            bail!("--config-path is given twice");
        }
    }
    Ok(path)
}

/// Stops on Ctrl+C (what the Dockerfile sends) or SIGTERM.
async fn stop() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(term) => term,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// `RUST_LOG` at `info` unless set. The relay's own logs never hold a code, nameplate or ticket;
/// the meeting place logs a refusal's code and nothing else.
fn logs() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .try_init();
}
