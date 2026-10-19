//! `mscp`, the parallel-TCP baseline from ADR 0010, timed from the sender to the receiver.

use std::time::{Duration, Instant};

use anyhow::{Result, ensure};
use serde_json::json;

use crate::{
    ceilings::{self, not_run},
    host::TimedOut,
    scenarios::{Scenario, Workload},
    session::{Session, generous},
    summary::{Row, RunResult, Status},
};

const MIB: u64 = 1 << 20;

/// Why `mscp` cannot run, if it cannot: a host lacks it, the receiver is `local`, or this is a
/// smoke run. `mscp` copies over ssh, so it needs a destination to copy to.
pub fn unavailable(s: &Session) -> Option<String> {
    ceilings::unavailable(s, "mscp", true, true).or_else(|| {
        s.receiver.is_local().then(|| {
            "the receiver is `local`, so mscp has no ssh destination to copy to".to_string()
        })
    })
}

/// Copies one file of the workload's size with `mscp`, `runs` times, and adds a row for it.
/// If either host lacks `mscp`, or the receiver is `local`, it records why and adds nothing.
pub async fn measure(
    s: &mut Session,
    scenario: &Scenario,
    workload: &Workload,
    rows: &mut Vec<Row>,
    notes: &mut Vec<String>,
) -> Result<()> {
    if let Some(reason) = unavailable(s) {
        notes.push(not_run(s, scenario, "mscp", &reason)?);
        return Ok(());
    }
    let bytes = workload.bytes()?;
    let source = s.sender.data_path("bench-mscp-src.bin");
    let target = s.receiver.data_path("bench-mscp-dst.bin");
    let outcome = copy_runs(s, scenario, workload, bytes, &source, &target).await;
    // Both files are the harness's own, so they go even after a failure.
    let _ = s.sender.remove_file(&source).await;
    let _ = s.receiver.remove_file(&target).await;
    rows.push(Row {
        workload: workload.name.clone(),
        name: format!("mscp -n {}", scenario.mscp_connections),
        reads_disk: true,
        writes_disk: true,
        runs: outcome?,
    });
    Ok(())
}

async fn copy_runs(
    s: &mut Session,
    scenario: &Scenario,
    workload: &Workload,
    bytes: u64,
    source: &str,
    target: &str,
) -> Result<Vec<RunResult>> {
    // Zeros are fine: ssh does not compress, so they cost the same on the wire as any other bytes.
    let make = format!(
        "dd if=/dev/zero of={} bs=1M count={} 2>/dev/null",
        crate::host::quote(source),
        bytes.div_ceil(MIB)
    );
    let made = s.sender.run(&["sh", "-c", &make], generous(bytes)).await?;
    ensure!(
        made.success(),
        "could not create {source}: {}",
        made.stderr.trim()
    );

    let destination = s.receiver.destination().expect("checked above");
    let remote = format!("{destination}:{target}");
    let connections = scenario.mscp_connections.to_string();
    let limit = Duration::from_secs(scenario.timeout_s + 30);
    let mut results = Vec::new();
    for run in 1..=s.runs {
        let started = Instant::now();
        let copied = s
            .sender
            .run(&["mscp", "-n", &connections, source, &remote], limit)
            .await;
        let elapsed = started.elapsed().as_secs_f64();
        let (status, error) = match &copied {
            Ok(out) if out.success() => (Status::Ok, None),
            Ok(out) => (Status::Failed, Some(out.stderr.trim().to_string())),
            Err(e) if e.downcast_ref::<TimedOut>().is_some() => (Status::Timeout, None),
            Err(e) => (Status::Failed, Some(format!("{e:#}"))),
        };
        if status == Status::Failed {
            s.failed += 1;
        }
        let gbit_s = (status == Status::Ok).then(|| bytes as f64 * 8.0 / 1e9 / elapsed);
        s.record(&json!({
            "kind": "mscp",
            "scenario": scenario.id,
            "workload": workload.name,
            "config": { "connections": scenario.mscp_connections, "size_bytes": bytes },
            "run": run,
            "status": format!("{status:?}").to_lowercase(),
            "error": error,
            "elapsed_s": elapsed,
            "throughput_gbit_s": gbit_s,
        }))?;
        results.push(RunResult {
            status,
            gbit_s,
            progress: None,
        });
        // The next run must not find the last one's file, which mscp may refuse to overwrite.
        s.receiver.remove_file(target).await?;
    }
    Ok(results)
}
