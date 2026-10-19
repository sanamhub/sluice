//! `bench smoke`: the whole harness on this machine, with the probe only, small enough for CI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};

use crate::{
    hosts::{Entry, Hosts},
    results::Results,
    runner, scenarios,
    session::Plan,
    summary::Status,
};

/// Runs as a scenario like any other, so a smoke run exercises the same code as `bench run`.
const SCENARIO: &str = r#"
[[scenario]]
id = "SMOKE"
link = "both ends on this host, loopback, no netem"
timeout_s = 40
workloads = [{ name = "smoke", size = "64MiB" }]
probe = [
  { mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared" },
  { mode = "raw", congestion = "cubic", window = "default", connections = 1, endpoints = "shared" },
  { mode = "blobs", congestion = "bbr3", window = "default", connections = 2, endpoints = "shared" },
]
"#;

/// Two runs per configuration is the least that gives a median and a spread.
const RUNS: u32 = 2;

/// Runs the smoke scenario. `probe` defaults to the `probe` binary next to this one, where cargo
/// puts it when the whole workspace is built. `out` is the results directory, which must not
/// exist, and defaults to one under the system temporary directory.
pub async fn run(probe: Option<PathBuf>, out: Option<PathBuf>) -> Result<bool> {
    let probe = match probe {
        Some(path) => path,
        None => std::env::current_exe()
            .context("find the bench binary")?
            .with_file_name(format!("probe{}", std::env::consts::EXE_SUFFIX)),
    };
    ensure!(
        probe.is_file(),
        "no probe at {}. Build it with `cargo build -p sluice-probe`, or pass --probe.",
        probe.display()
    );
    let temp = std::env::temp_dir();
    let data_dir = temp.join(format!("sluice-bench-smoke-data-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).context("create the data directory")?;
    let results_dir =
        out.unwrap_or_else(|| temp.join(format!("sluice-bench-smoke-{}", std::process::id())));
    let results = Results::create_in(results_dir)?;

    let entry = Entry {
        host: "local".to_string(),
        interface: None,
        address: None,
        probe: probe.display().to_string(),
        data_dir: data_dir.display().to_string(),
    };
    let hosts = Hosts {
        sender: entry.clone(),
        receiver: entry,
    };
    let scenarios = scenarios::parse(SCENARIO)?;
    let plan = Plan {
        ceilings_and_baseline: false,
        size_override: None,
    };
    let finished = runner::run(&hosts, &scenarios, results, "smoke", RUNS, plan).await;
    let _ = std::fs::remove_dir_all(&data_dir);
    let finished = finished?;

    let not_ok: Vec<String> = finished
        .report
        .scenarios
        .iter()
        .flat_map(|s| &s.rows)
        .filter(|row| row.runs.iter().any(|r| r.status != Status::Ok))
        .map(|row| row.name.clone())
        .collect();
    if !not_ok.is_empty() {
        bail!("smoke runs did not all finish: {}", not_ok.join(", "));
    }
    println!("results: {}", finished.dir.display());
    Ok(finished.clean)
}
