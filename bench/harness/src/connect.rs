//! `bench connect`: how often two hosts connect when a rendezvous server is all the infrastructure
//! they have (ADR 0013).

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};

use crate::{
    connect_summary::{self, Attempt, Ended, Report},
    env,
    host::{Ctx, Host, Spawned},
    hosts::Hosts,
    rendezvous::{PortMapping, Rendezvous, learned, ticket_discovery},
    results::{Results, today},
    tools::{self, found},
};

/// Bytes moved once the connection is up. Enough to show the connection carries data, and small so
/// that 40 attempts take minutes.
const SIZE: &str = "16MiB";
/// `fetch --timeout`, which is also how long a dial may take before it is `no_path`.
const TIMEOUT_S: u64 = 30;
/// `serve` waits up to 10 s for the server before it prints the ticket.
const TICKET_WAIT: Duration = Duration::from_secs(60);
/// Both ends wait up to 10 s for the server, the dial may take `TIMEOUT_S`, the transfer may take
/// another `TIMEOUT_S`, and this is the slack on top.
const FETCH_LIMIT: Duration = Duration::from_secs(10 + 2 * TIMEOUT_S + 20);
/// How long to wait for `serve`'s line about a connection that closed.
const LINES_WAIT: Duration = Duration::from_secs(20);

/// What a finished run leaves for the caller.
pub struct Finished {
    /// The results directory.
    pub dir: PathBuf,
    /// True if every attempt produced a result line.
    pub clean: bool,
}

/// Runs the attempts and writes `raw.jsonl`, `env.json` and `summary.md`. The files are written
/// even if the run is interrupted, so what was measured is kept.
pub async fn run(
    hosts: &Hosts,
    rendezvous: &Rendezvous,
    mut results: Results,
    label: &str,
    runs: u32,
) -> Result<Finished> {
    let ctx = Ctx::default();
    let sender = Host::connect("sender", &hosts.sender, &ctx).await?;
    let receiver = Host::connect("receiver", &hosts.receiver, &ctx).await?;
    let sender_tools = tools::detect(&sender).await?;
    let receiver_tools = tools::detect(&receiver).await?;
    for (host, tools) in [(&sender, &sender_tools), (&receiver, &receiver_tools)] {
        ensure!(
            found(tools, "probe").is_some(),
            "the probe is not runnable on the {} at {}. `bench check` shows what is missing.",
            host.role,
            host.entry.probe
        );
    }

    let mut attempts = Vec::new();
    let mut clean = true;
    let outcome = {
        let work = async {
            for run in 1..=runs {
                // Interleaved, so a network that changes during the hours the run takes does not
                // land on one setting.
                for setting in [PortMapping::Off, PortMapping::On] {
                    let taken = attempt(&sender, &receiver, rendezvous, setting).await;
                    let (attempt, mut record) = match taken {
                        Ok(taken) => taken,
                        Err(e) => failed(setting, &format!("{e:#}")),
                    };
                    clean &= attempt.ended != Ended::Other;
                    record["label"] = json!(label);
                    record["run"] = json!(run);
                    results.record(&record)?;
                    println!(
                        "  run {run}, port mapping {}: {}",
                        setting.word(),
                        describe(&attempt)
                    );
                    attempts.push(attempt);
                }
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::select! {
            done = work => done,
            _ = tokio::signal::ctrl_c() => Err(anyhow!("interrupted")),
        }
    };
    ctx.reap_remote().await;

    let hosts_env = vec![
        env::host(&sender, &sender_tools).await,
        env::host(&receiver, &receiver_tools).await,
    ];
    let document = env::document(hosts_env, &ctx.commands());
    results.write("env.json", &serde_json::to_string_pretty(&document)?)?;
    let report = Report {
        label: label.to_string(),
        date: today(),
        hosts: vec![describe_host(&sender), describe_host(&receiver)],
        rendezvous: rendezvous.url().to_string(),
        runs,
        attempts,
    };
    results.write("summary.md", &connect_summary::render(&report))?;
    outcome?;
    Ok(Finished {
        dir: results.dir().to_path_buf(),
        clean,
    })
}

fn describe_host(host: &Host) -> String {
    format!("{}: {} ({:?})", host.role, host.entry.host, host.os)
}

fn describe(attempt: &Attempt) -> String {
    match (attempt.ended, attempt.connect_s) {
        (Ended::Connected, Some(s)) => format!("connected in {s:.2} s"),
        (Ended::Connected, None) => "connected".to_string(),
        (Ended::NoPath, _) => "no_path".to_string(),
        (Ended::Other, _) => "no result".to_string(),
    }
}

/// What a run that could not be carried out looks like in `raw.jsonl` and in the summary.
fn failed(setting: PortMapping, error: &str) -> (Attempt, Value) {
    let attempt = Attempt {
        port_mapping: setting,
        ended: Ended::Other,
        connect_s: None,
        sender_learned: Vec::new(),
        receiver_learned: Vec::new(),
    };
    let record = json!({
        "kind": "connect",
        "port_mapping": setting.word(),
        "status": "failed",
        "error": error,
        "ticket": null,
        "fetch": null,
        "serve": [],
    });
    (attempt, record)
}

/// One `serve` and `fetch` pair.
async fn attempt(
    sender: &Host,
    receiver: &Host,
    rendezvous: &Rendezvous,
    setting: PortMapping,
) -> Result<(Attempt, Value)> {
    let mut argv = vec![
        sender.entry.probe.clone(),
        "serve".to_string(),
        "--mode".to_string(),
        "raw".to_string(),
        "--size".to_string(),
        SIZE.to_string(),
    ];
    argv.extend(rendezvous.args(setting));
    let mut serve = sender.spawn(&argv)?;
    let driven = drive(receiver, rendezvous, setting, &mut serve).await;
    serve.kill().await;
    let _ = serve.finish(Duration::from_secs(5)).await;
    driven
}

async fn drive(
    receiver: &Host,
    rendezvous: &Rendezvous,
    setting: PortMapping,
    serve: &mut Spawned,
) -> Result<(Attempt, Value)> {
    let ticket = serve
        .next_line(TICKET_WAIT)
        .await
        .context("wait for the ticket")?
        .with_context(|| {
            format!(
                "serve exited before printing a ticket: {}",
                serve.stderr_tail()
            )
        })?;
    let sender_discovery = ticket_discovery(&ticket);

    let mut argv = vec![
        receiver.entry.probe.clone(),
        "fetch".to_string(),
        ticket.clone(),
        "--mode".to_string(),
        "raw".to_string(),
        "--timeout".to_string(),
        TIMEOUT_S.to_string(),
    ];
    argv.extend(rendezvous.args(setting));
    let mut fetch = receiver.spawn(&argv)?;
    let started = Instant::now();
    let mut result = None;
    let mut outlived = false;
    loop {
        let left = FETCH_LIMIT.saturating_sub(started.elapsed());
        match fetch.next_line(left).await {
            Ok(Some(line)) => result = serde_json::from_str::<Value>(&line).ok().or(result),
            Ok(None) => break,
            Err(_) => {
                outlived = true;
                break;
            }
        }
    }
    if outlived {
        fetch.kill().await;
    }
    let finished = fetch.finish(Duration::from_secs(30)).await?;

    let outcome = result.as_ref().and_then(|r| r["outcome"].as_str());
    let ended = match outcome {
        Some("ok" | "timeout") => Ended::Connected,
        Some("no_path") => Ended::NoPath,
        _ => Ended::Other,
    };
    // A connection that came up closes on `serve` too, and its line says which addresses it used.
    let mut lines = Vec::new();
    if ended == Ended::Connected
        && let Ok(Some(line)) = serve.next_line(LINES_WAIT).await
    {
        lines.extend(serde_json::from_str::<Value>(&line).ok());
    }
    let receiver_discovery = result.as_ref().map(|r| r["discovery"].clone());
    let attempt = Attempt {
        port_mapping: setting,
        ended,
        connect_s: result.as_ref().and_then(|r| r["connect_s"].as_f64()),
        sender_learned: learned(sender_discovery.as_ref()),
        receiver_learned: learned(receiver_discovery.as_ref().filter(|d| !d.is_null())),
    };
    let status = match (outcome, outlived) {
        (_, true) => "failed",
        (Some("ok"), _) => "ok",
        (Some("timeout"), _) => "timeout",
        (Some("no_path"), _) => "no_path",
        _ => "failed",
    };
    let mut record = json!({
        "kind": "connect",
        "port_mapping": setting.word(),
        "status": status,
        "ticket": ticket,
        "ticket_discovery": sender_discovery,
        "fetch": result,
        "fetch_exit": finished.code,
        "harness_killed": outlived || finished.killed,
        "serve": lines,
    });
    if ended != Ended::Connected {
        record["stderr"] = json!({ "serve": serve.stderr_tail(), "fetch": fetch.stderr_tail() });
    }
    Ok((attempt, record))
}
