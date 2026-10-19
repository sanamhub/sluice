//! One probe configuration: start `serve`, read the ticket, run `fetch`, collect both sides.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use serde_json::{Value, json};

use crate::{
    host::Spawned,
    probe_cmd::{KILL_AFTER, classify, fetch_args, join, progress_bytes, result_of, serve_args},
    scenarios::{ProbeConfig, Scenario, Workload},
    session::Session,
    stall::StallWatch,
    summary::{Row, RunResult, Status},
};

/// Longest `serve` may take to print its ticket. Importing 100 GiB into a file store is most of
/// it.
const READY: Duration = Duration::from_secs(1800);
/// How long to wait for `serve`'s connection lines once `fetch` has exited.
const LINES_WAIT: Duration = Duration::from_secs(20);
/// How long to wait for them when `fetch` was killed and `serve` could not be interrupted: the
/// connection closes when it has been idle for iroh's 30 s timeout.
const IDLE_WAIT: Duration = Duration::from_secs(45);

static SINKS: AtomicU64 = AtomicU64::new(0);

/// Runs `config` `s.runs` times against `workload` and returns the row for the summary.
pub async fn run_config(
    s: &mut Session,
    scenario: &Scenario,
    workload: &Workload,
    config: &ProbeConfig,
) -> Result<Row> {
    println!("  {} / {}", workload.name, config.label());
    let mut runs = Vec::new();
    for run in 1..=s.runs {
        let sink_dir = config.writes_disk().then(|| {
            s.receiver.data_path(&format!(
                "bench-sink-{}-{}",
                std::process::id(),
                SINKS.fetch_add(1, Ordering::Relaxed)
            ))
        });
        let taken = attempt(s, scenario, workload, config, sink_dir.as_deref()).await;
        if let Some(dir) = &sink_dir {
            // The directory is the harness's own, so it goes even after a failure.
            let _ = s.receiver.remove_dir_all(dir).await;
        }
        let (status, error, result, record) = match taken {
            Ok(a) => {
                let (status, error) =
                    classify(a.fetch.as_ref(), a.killed, a.stalled, s.receiver.os);
                let result = result_of(status, a.fetch.as_ref(), &a.live);
                let record = a.record(status);
                (status, error, result, record)
            }
            Err(e) => {
                let error = format!("{e:#}");
                let record =
                    json!({ "fetch": null, "serve": [], "joined": [], "harness_killed": false });
                (
                    Status::Failed,
                    Some(error),
                    RunResult {
                        status: Status::Failed,
                        gbit_s: None,
                        progress: None,
                    },
                    record,
                )
            }
        };
        if status == Status::Failed {
            s.failed += 1;
        }
        let mut line = json!({
            "kind": "probe",
            "scenario": scenario.id,
            "workload": workload.name,
            "config": config_json(config),
            "run": run,
            "status": format!("{status:?}").to_lowercase(),
            "error": error,
        });
        line.as_object_mut()
            .expect("object")
            .extend(record.as_object().expect("object").clone());
        s.record(&line)?;
        println!("    run {run}: {}", describe(status, &line));
        runs.push(result);
    }
    Ok(Row {
        workload: workload.name.clone(),
        name: config.label(),
        reads_disk: config.reads_disk(),
        writes_disk: config.writes_disk(),
        runs,
    })
}

fn describe(status: Status, line: &Value) -> String {
    let gbit = line["fetch"]["throughput_gbit_s"].as_f64();
    match (status, gbit) {
        (Status::Ok, Some(g)) => format!("{g:.2} Gbit/s"),
        (Status::Failed, _) => format!("failed: {}", line["error"].as_str().unwrap_or("")),
        _ => format!("{status:?}").to_lowercase(),
    }
}

fn config_json(config: &ProbeConfig) -> Value {
    let mut value = serde_json::to_value(config).expect("config serializes");
    value["label"] = Value::String(config.label());
    value
}

/// What one run left behind.
struct Attempt {
    fetch: Option<Value>,
    fetch_exit: Option<i32>,
    killed: bool,
    /// Seconds waited before the harness stopped `fetch` for stalling, if it did.
    stalled: Option<u64>,
    /// Cumulative bytes from the progress lines, the only record of a run that was stopped.
    live: Vec<u64>,
    serve: Vec<Value>,
    serve_stderr: String,
    fetch_stderr: String,
}

impl Attempt {
    fn record(&self, status: Status) -> Value {
        let joined = self
            .fetch
            .as_ref()
            .map(|f| join(f, &self.serve))
            .unwrap_or_default();
        let mut record = json!({
            "harness_killed": self.killed,
            "stalled": self.stalled.is_some(),
            "fetch_exit": self.fetch_exit,
            "fetch": self.fetch,
            "serve": self.serve,
            "joined": joined,
        });
        if self.fetch.is_none() {
            record["progress_lines"] = json!(self.live);
        }
        if status != Status::Ok {
            record["stderr"] = json!({ "serve": self.serve_stderr, "fetch": self.fetch_stderr });
        }
        record
    }
}

async fn attempt(
    s: &Session,
    scenario: &Scenario,
    workload: &Workload,
    config: &ProbeConfig,
    sink_dir: Option<&str>,
) -> Result<Attempt> {
    let argv = serve_args(&s.sender, config, &workload.size);
    let mut serve = s.sender.spawn(&argv)?;
    let driven = drive(s, scenario, config, sink_dir, &mut serve).await;
    serve.kill().await;
    let _ = serve.finish(Duration::from_secs(5)).await;
    driven
}

async fn drive(
    s: &Session,
    scenario: &Scenario,
    config: &ProbeConfig,
    sink_dir: Option<&str>,
    serve: &mut Spawned,
) -> Result<Attempt> {
    let first = serve
        .next_line(READY)
        .await
        .context("wait for the ticket")?;
    let ticket = first.with_context(|| {
        format!(
            "serve exited before printing a ticket: {}",
            serve.stderr_tail()
        )
    })?;
    let argv = fetch_args(&s.receiver, config, &ticket, scenario.timeout_s, sink_dir);
    let mut fetch = s.receiver.spawn(&argv)?;
    let limit = Duration::from_secs(scenario.timeout_s + KILL_AFTER);
    let watched = watch(&mut fetch, scenario.stall_s, limit).await;
    if watched.stalled.is_some() || watched.outlived {
        fetch.kill().await;
    }
    let finished = fetch.finish(Duration::from_secs(KILL_AFTER)).await?;

    // serve prints a line as each connection closes, which fetch did on its way out. A fetch that
    // was killed closed nothing, so serve is asked to stop, which closes them.
    let stopped = watched.stalled.is_some();
    let interrupted = stopped && serve.interrupt().await;
    let wait = if stopped && !interrupted {
        IDLE_WAIT
    } else {
        LINES_WAIT
    };
    let wanted = usize::from(config.connections);
    let deadline = Instant::now() + wait;
    let mut lines = Vec::new();
    while lines.len() < wanted {
        let left = deadline.saturating_duration_since(Instant::now());
        match serve.next_line(left).await {
            Ok(Some(line)) => lines.extend(serde_json::from_str::<Value>(&line).ok()),
            _ => break,
        }
    }
    Ok(Attempt {
        fetch: watched.result,
        fetch_exit: finished.code,
        killed: watched.outlived || finished.killed,
        stalled: watched.stalled,
        live: watched.live,
        serve: lines,
        serve_stderr: serve.stderr_tail(),
        fetch_stderr: fetch.stderr_tail(),
    })
}

/// What `fetch` printed, and whether it was stopped.
struct Watched {
    /// The result line, the last line that is not a progress event.
    result: Option<Value>,
    /// The cumulative bytes of each progress line.
    live: Vec<u64>,
    stalled: Option<u64>,
    /// `fetch` was still running `limit` after it started, so the harness will kill it.
    outlived: bool,
}

/// Reads `fetch`'s stdout until it closes, stopping early if the progress lines show a stall or
/// `limit` passes. It does not kill anything: the caller does, because it also has `serve` to deal
/// with.
async fn watch(fetch: &mut Spawned, stall_s: u64, limit: Duration) -> Watched {
    let deadline = Instant::now() + limit;
    let mut stall = StallWatch::new(stall_s);
    let mut watched = Watched {
        result: None,
        live: Vec::new(),
        stalled: None,
        outlived: false,
    };
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match fetch.next_line(left).await {
            Ok(Some(line)) => match progress_bytes(&line) {
                Some(bytes) => {
                    watched.live.push(bytes);
                    if stall.push(bytes) {
                        watched.stalled = Some(stall_s);
                        break;
                    }
                }
                None => {
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        watched.result = Some(value);
                    }
                }
            },
            Ok(None) => break,
            Err(_) => {
                watched.outlived = true;
                break;
            }
        }
    }
    watched
}

#[cfg(all(test, unix))]
mod tests;
