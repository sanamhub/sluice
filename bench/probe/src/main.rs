//! Transport probe for iroh and iroh-blobs. Output is for machines: one ticket line from `serve`,
//! one JSON line from `fetch`, everything else on stderr.

mod blobs;
mod cli;
mod cpu;
mod data;
mod fetch;
mod net;
mod raw;
mod result;
mod serve;
mod ticket;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    match Cli::parse().command {
        Command::Serve(args) => serve::run(args).await,
        Command::Fetch(args) => {
            println!("{}", fetch::run(args).await?.line()?);
            Ok(())
        }
    }
}
