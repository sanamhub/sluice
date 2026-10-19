//! The ceilings of a path and its disks: `iperf3` for the network, `fio` for the disks.

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    host::{Host, Os, Spawned},
    scenarios::Scenario,
    session::Session,
    summary::Ceilings,
    tools::found,
};

const IPERF_PORT: &str = "5201";
const IPERF_SECONDS: &str = "10";
const PARALLEL_STREAMS: u32 = 8;
const FIO_SECONDS: &str = "15";
/// Large enough that the cache is not the whole file, small enough for any data directory.
const FIO_SIZE: &str = "4GiB";
const LISTEN_WAIT: Duration = Duration::from_secs(30);

/// Which disk ceilings the scenario's runs need.
#[derive(Clone, Copy)]
pub struct Wanted {
    /// Some run reads the blob from the sender's disk.
    pub read: bool,
    /// Some run writes the blob to the receiver's disk.
    pub write: bool,
}

/// Measures the ceilings of the path as it is now, with netem applied if the scenario has it.
/// Anything that cannot run is recorded with its reason and added to `notes`.
pub async fn measure(
    s: &mut Session,
    scenario: &Scenario,
    wanted: Wanted,
    notes: &mut Vec<String>,
) -> Result<Ceilings> {
    let mut ceilings = Ceilings::default();
    if let Some(reason) = unavailable(s, "iperf3", true, true) {
        notes.push(not_run(s, scenario, "iperf3", &reason)?);
    } else {
        network(s, scenario, &mut ceilings, notes).await?;
    }
    if wanted.read {
        ceilings.fio_read = disk(s, scenario, true, notes).await?;
    }
    if wanted.write {
        ceilings.fio_write = disk(s, scenario, false, notes).await?;
    }
    if !wanted.read && !wanted.write {
        let reason = unavailable(s, "fio", true, true)
            .unwrap_or_else(|| "no configuration reads or writes a disk".to_string());
        notes.push(not_run(s, scenario, "fio", &reason)?);
    }
    Ok(ceilings)
}

/// Why `name` cannot run, if it cannot: it is missing on a host that needs it, or the run is a
/// smoke run that uses the probe only.
pub fn unavailable(s: &Session, name: &str, on_sender: bool, on_receiver: bool) -> Option<String> {
    let mut missing = Vec::new();
    if on_sender && found(&s.sender_tools, name).is_none() {
        missing.push("the sender");
    }
    if on_receiver && found(&s.receiver_tools, name).is_none() {
        missing.push("the receiver");
    }
    if !missing.is_empty() {
        return Some(format!("{name} not found on {}", missing.join(" or ")));
    }
    (!s.plan.ceilings_and_baseline).then(|| "bench smoke runs the probe only".to_string())
}

/// Records why `name` did not run. A smoke run's reason is a choice, anything else is missing.
pub fn not_run(s: &mut Session, scenario: &Scenario, name: &str, reason: &str) -> Result<String> {
    let status = if reason.contains("not found") {
        "missing"
    } else {
        "skipped"
    };
    s.record_not_run(&scenario.id, name, status, reason)
}

async fn network(
    s: &mut Session,
    scenario: &Scenario,
    ceilings: &mut Ceilings,
    notes: &mut Vec<String>,
) -> Result<()> {
    let argv = ["iperf3", "-s", "-p", IPERF_PORT].map(String::from);
    let mut server = s.receiver.spawn(&argv)?;
    let ran = iperf_runs(s, scenario, &mut server, ceilings, notes).await;
    server.kill().await;
    let _ = server.finish(Duration::from_secs(5)).await;
    ran
}

async fn iperf_runs(
    s: &mut Session,
    scenario: &Scenario,
    server: &mut Spawned,
    ceilings: &mut Ceilings,
    notes: &mut Vec<String>,
) -> Result<()> {
    wait_until_listening(server).await?;
    let address = s.receiver.entry.reachable_at();
    for streams in [1, PARALLEL_STREAMS] {
        for run in 1..=s.runs {
            let bits = iperf_once(s, scenario, &address, streams, run).await?;
            match bits {
                Some(bits) if streams == 1 => ceilings.iperf_single.push(bits),
                Some(bits) => ceilings.iperf_parallel.push(bits),
                None => notes.push(format!(
                    "iperf3 with {streams} streams, run {run}: no result"
                )),
            }
        }
    }
    Ok(())
}

async fn wait_until_listening(server: &mut Spawned) -> Result<()> {
    for _ in 0..5 {
        match server.next_line(LISTEN_WAIT).await? {
            Some(line) if line.contains("listening") => return Ok(()),
            Some(_) => {}
            None => break,
        }
    }
    anyhow::bail!("iperf3 server did not start: {}", server.stderr_tail())
}

/// One client run, retried while the server finishes the previous test.
async fn iperf_once(
    s: &mut Session,
    scenario: &Scenario,
    address: &str,
    streams: u32,
    run: u32,
) -> Result<Option<f64>> {
    let streams_arg = streams.to_string();
    let argv = [
        "iperf3",
        "-c",
        address,
        "-p",
        IPERF_PORT,
        "-P",
        &streams_arg,
        "-t",
        IPERF_SECONDS,
        "-J",
    ];
    let (mut result, mut error) = (Value::Null, String::new());
    for _ in 0..4 {
        let out = s.sender.run(&argv, Duration::from_secs(90)).await?;
        result = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
        error = result["error"]
            .as_str()
            .map_or_else(|| out.stderr.trim().to_string(), str::to_string);
        if result["end"]["sum_received"]["bits_per_second"].is_f64() || !error.contains("busy") {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let bits = result["end"]["sum_received"]["bits_per_second"].as_f64();
    if bits.is_none() {
        s.failed += 1;
    }
    s.record(&json!({
        "kind": "iperf3",
        "scenario": scenario.id,
        "config": { "streams": streams, "seconds": IPERF_SECONDS },
        "run": run,
        "status": if bits.is_some() { "ok" } else { "failed" },
        "error": bits.is_none().then_some(error),
        "bits_per_second": bits,
        "result": result,
    }))?;
    Ok(bits)
}

/// Sequential read on the sender or write on the receiver, once per run. Returns bytes per
/// second for each run that produced a number.
async fn disk(
    s: &mut Session,
    scenario: &Scenario,
    read: bool,
    notes: &mut Vec<String>,
) -> Result<Vec<f64>> {
    let host = if read {
        s.sender.clone()
    } else {
        s.receiver.clone()
    };
    if let Some(reason) = unavailable_on(s, &host, "fio") {
        notes.push(not_run(
            s,
            scenario,
            "fio",
            &format!("{reason} ({})", mode_name(read)),
        )?);
        return Ok(Vec::new());
    }
    let file = host.data_path("bench-fio.dat");
    let mut rates = Vec::new();
    let mut outcome = Ok(());
    for run in 1..=s.runs {
        match fio_once(s, scenario, &host, &file, read, run).await {
            Ok(Some(rate)) => rates.push(rate),
            Ok(None) => notes.push(format!("fio {}, run {run}: no result", mode_name(read))),
            Err(error) => {
                outcome = Err(error);
                break;
            }
        }
    }
    // The file is the harness's own, so it goes even if a run failed.
    let _ = host.remove_file(&file).await;
    outcome.map(|()| rates)
}

fn mode_name(read: bool) -> &'static str {
    if read {
        "read on the sender"
    } else {
        "write on the receiver"
    }
}

fn unavailable_on(s: &Session, host: &Host, name: &str) -> Option<String> {
    let tools = if host.role == "sender" {
        &s.sender_tools
    } else {
        &s.receiver_tools
    };
    if found(tools, name).is_none() {
        return Some(format!("{name} not found on the {}", host.role));
    }
    (!s.plan.ceilings_and_baseline).then(|| "bench smoke runs the probe only".to_string())
}

fn engine(os: Os) -> [&'static str; 3] {
    match os {
        Os::Linux => ["--ioengine=libaio", "--direct=1", "--iodepth=32"],
        Os::Windows => ["--ioengine=windowsaio", "--direct=1", "--iodepth=32"],
        // Without O_DIRECT on macOS the page cache inflates a read, so the engine is all we ask.
        _ => ["--ioengine=posixaio", "--iodepth=32", "--bs=1M"],
    }
}

async fn fio_once(
    s: &mut Session,
    scenario: &Scenario,
    host: &Host,
    file: &str,
    read: bool,
    run: u32,
) -> Result<Option<f64>> {
    let rw = if read { "read" } else { "write" };
    let name = format!("--name=bench-{rw}");
    let filename = format!("--filename={file}");
    let mut argv: Vec<&str> = vec![
        "fio",
        &name,
        &filename,
        "--bs=1M",
        "--size",
        FIO_SIZE,
        "--time_based",
        "--runtime",
        FIO_SECONDS,
        "--output-format=json",
    ];
    argv.push(if read { "--rw=read" } else { "--rw=write" });
    if !read {
        argv.push("--end_fsync=1");
    }
    argv.extend(engine(host.os));
    let out = host.run(&argv, Duration::from_secs(600)).await?;
    let result: Value = serde_json::from_str(&out.stdout).unwrap_or(Value::Null);
    let job = &result["jobs"][0][rw];
    // Older fio prints only `bw`, in KiB/s.
    let rate = job["bw_bytes"]
        .as_f64()
        .or_else(|| job["bw"].as_f64().map(|kib| kib * 1024.0));
    if rate.is_none() {
        s.failed += 1;
    }
    s.record(&json!({
        "kind": "fio",
        "scenario": scenario.id,
        "config": { "host": host.role, "rw": rw, "size": FIO_SIZE, "seconds": FIO_SECONDS },
        "run": run,
        "status": if rate.is_some() { "ok" } else { "failed" },
        "error": rate.is_none().then(|| out.stderr.trim().to_string()),
        "bytes_per_second": rate,
        "result": result,
    }))?;
    Ok(rate)
}

impl Wanted {
    /// The disk ceilings a scenario's configurations and the `mscp` baseline need.
    pub fn of(scenario: &Scenario, baseline: bool) -> Self {
        let any = |f: fn(&crate::scenarios::ProbeConfig) -> bool| scenario.probe.iter().any(f);
        Self {
            read: baseline || any(|p| p.reads_disk()),
            write: baseline || any(|p| p.writes_disk()),
        }
    }
}
