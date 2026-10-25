//! `bench`: drives `probe` on a pair of hosts for the measurements and writes results anyone can rerun.

mod ceilings;
mod check;
mod connect;
mod connect_summary;
mod env;
mod host;
mod hosts;
mod mscp;
mod netem;
mod probe_cmd;
mod probe_run;
mod rendezvous;
mod results;
mod runner;
mod scenarios;
mod session;
mod size_override;
mod smoke;
mod stall;
mod stats;
mod summary;
mod tools;

use std::{path::PathBuf, process::ExitCode};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use crate::{results::Results, session::Plan};

/// The transport benchmark.
#[derive(Parser)]
#[command(name = "bench", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report what each host has and lacks. Changes nothing.
    Check {
        /// Hosts file, see bench/hosts.example.toml.
        #[arg(long)]
        hosts: PathBuf,
        /// A rendezvous server, https://host[:udp-port]. Adds, for each host, whether the name
        /// resolves and whether the server answers over UDP.
        #[arg(long)]
        rendezvous: Option<String>,
        /// A PEM file with the server's root certificate, at this path on every host.
        #[arg(long, requires = "rendezvous")]
        rendezvous_ca: Option<String>,
    },
    /// Measure how often the two hosts connect with a rendezvous server and no relay, and write
    /// bench/results/<UTC date>-<label>/.
    Connect {
        /// Hosts file, see bench/hosts.example.toml.
        #[arg(long)]
        hosts: PathBuf,
        /// A rendezvous server, https://host[:udp-port].
        #[arg(long)]
        rendezvous: String,
        /// A PEM file with the server's root certificate, at this path on both hosts. A server with
        /// a LetsEncrypt certificate needs none.
        #[arg(long)]
        rendezvous_ca: Option<String>,
        /// Names the results directory. Letters, digits, dot, dash and underscore.
        #[arg(long)]
        label: String,
        /// Attempts per port mapping setting.
        #[arg(long, default_value_t = 20)]
        runs: u32,
        /// Where the results directory goes.
        #[arg(long, default_value = "bench/results")]
        out_dir: PathBuf,
    },
    /// Run the scenarios and write bench/results/<UTC date>-<label>/.
    Run {
        /// Hosts file, see bench/hosts.example.toml.
        #[arg(long)]
        hosts: PathBuf,
        /// Names the results directory. Letters, digits, dot, dash and underscore.
        #[arg(long)]
        label: String,
        /// Run one scenario, for example S1. Default: all of them.
        #[arg(long)]
        scenario: Option<String>,
        /// Runs per configuration.
        #[arg(long, default_value_t = 5)]
        runs: u32,
        /// Replaces every workload's size for this run, for example 20GiB. The summary says so.
        #[arg(long)]
        size: Option<String>,
        /// The scenarios file.
        #[arg(long, default_value = "bench/scenarios.toml")]
        scenarios: PathBuf,
        /// Where the results directory goes.
        #[arg(long, default_value = "bench/results")]
        out_dir: PathBuf,
    },
    /// Both ends on this machine, no netem, small sizes. For CI.
    Smoke {
        /// The probe binary. Default: the one next to this binary.
        #[arg(long)]
        probe: Option<PathBuf>,
        /// The results directory, which must not exist. Default: one in the temporary directory.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match execute(Cli::parse()).await {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

/// True if everything the command did went cleanly.
async fn execute(cli: Cli) -> Result<bool> {
    match cli.command {
        Command::Check {
            hosts,
            rendezvous,
            rendezvous_ca,
        } => {
            let rendezvous = rendezvous
                .map(|url| rendezvous::Rendezvous::parse(&url, rendezvous_ca))
                .transpose()?;
            check::run(&hosts::load(&hosts)?, rendezvous.as_ref()).await
        }
        Command::Connect {
            hosts,
            rendezvous,
            rendezvous_ca,
            label,
            runs,
            out_dir,
        } => {
            if runs == 0 {
                bail!("--runs must be at least 1");
            }
            let rendezvous = rendezvous::Rendezvous::parse(&rendezvous, rendezvous_ca)?;
            let hosts = hosts::load(&hosts)?;
            let results = Results::create(&out_dir, &label)?;
            let finished = connect::run(&hosts, &rendezvous, results, &label, runs).await?;
            println!("results: {}", finished.dir.display());
            Ok(finished.clean)
        }
        Command::Run {
            hosts,
            label,
            scenario,
            runs,
            size,
            scenarios,
            out_dir,
        } => {
            if runs == 0 {
                bail!("--runs must be at least 1");
            }
            let hosts = hosts::load(&hosts)?;
            let text = std::fs::read_to_string(&scenarios)
                .with_context(|| format!("read {}", scenarios.display()))?;
            let mut all = scenarios::parse(&text)?;
            if let Some(id) = scenario {
                all.retain(|s| s.id == id);
                if all.is_empty() {
                    bail!("{} has no scenario {id}", scenarios.display());
                }
            }
            // Before the results directory exists, so a bad size leaves nothing behind.
            let size_override = size
                .as_deref()
                .map(|size| size_override::apply(&mut all, size))
                .transpose()?;
            let results = Results::create(&out_dir, &label)?;
            let plan = Plan {
                ceilings_and_baseline: true,
                size_override,
            };
            let finished = runner::run(&hosts, &all, results, &label, runs, plan).await?;
            println!("results: {}", finished.dir.display());
            Ok(finished.clean)
        }
        Command::Smoke { probe, out } => smoke::run(probe, out).await,
    }
}
