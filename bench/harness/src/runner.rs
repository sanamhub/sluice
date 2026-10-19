//! `bench run`: every scenario in turn, with netem applied around it and removed after it.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::json;

use crate::{
    ceilings::{self, Wanted},
    env,
    host::{Ctx, Host, Os},
    hosts::Hosts,
    mscp, netem,
    netem::Shaping,
    probe_run,
    results::{Results, today},
    scenarios::Scenario,
    session::{Plan, Session},
    summary::{self, Ceilings, Report, ScenarioReport},
    tools::{self, found},
};

/// What a finished run leaves for the caller.
pub struct Finished {
    /// The results directory.
    pub dir: PathBuf,
    /// True if no run failed. Timeouts are results, not failures.
    pub clean: bool,
    /// Everything that went into `summary.md`.
    pub report: Report,
}

/// Runs `scenarios` and writes `raw.jsonl`, `env.json` and `summary.md`. The files are written
/// even if a scenario fails or the run is interrupted, so what was measured is kept, and the error
/// is returned after that.
pub async fn run(
    hosts: &Hosts,
    scenarios: &[Scenario],
    results: Results,
    label: &str,
    runs: u32,
    plan: Plan,
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

    let mut session = Session {
        sender,
        receiver,
        sender_tools,
        receiver_tools,
        results,
        runs,
        plan,
        failed: 0,
    };
    let mut report = Report {
        label: label.to_string(),
        date: today(),
        hosts: vec![describe(&session.sender), describe(&session.receiver)],
        runs,
        scenarios: Vec::new(),
    };
    let shaping = Shaping::default();

    let outcome = {
        let work = async {
            for scenario in scenarios {
                report.scenarios.push(empty(scenario));
                let current = report.scenarios.last_mut().expect("just pushed");
                run_scenario(&mut session, &shaping, scenario, current).await?;
            }
            Ok::<(), anyhow::Error>(())
        };
        tokio::select! {
            done = work => done,
            _ = tokio::signal::ctrl_c() => Err(anyhow!("interrupted")),
        }
    };
    // Whatever happened, the interface goes back to how it was and no remote process stays behind.
    let undone = shaping.undo().await;
    ctx.reap_remote().await;

    let hosts_env = vec![
        env::host(&session.sender, &session.sender_tools).await,
        env::host(&session.receiver, &session.receiver_tools).await,
    ];
    let document = env::document(hosts_env, &ctx.commands());
    session
        .results
        .write("env.json", &serde_json::to_string_pretty(&document)?)?;
    session
        .results
        .write("summary.md", &summary::render(&report))?;

    outcome?;
    undone.context("remove netem")?;
    Ok(Finished {
        dir: session.results.dir().to_path_buf(),
        clean: session.failed == 0,
        report,
    })
}

fn describe(host: &Host) -> String {
    format!("{}: {} ({:?})", host.role, host.entry.host, host.os)
}

fn empty(scenario: &Scenario) -> ScenarioReport {
    ScenarioReport {
        id: scenario.id.clone(),
        link: scenario.link.clone(),
        netem: None,
        skipped: None,
        ceilings: Ceilings::default(),
        rows: Vec::new(),
        not_measured: Vec::new(),
    }
}

async fn run_scenario(
    s: &mut Session,
    shaping: &Shaping,
    scenario: &Scenario,
    out: &mut ScenarioReport,
) -> Result<()> {
    println!("{}: {}", scenario.id, scenario.link);
    if let Some(wanted) = &scenario.netem {
        if let Some(reason) = cannot_shape(s) {
            s.record(&json!({ "kind": "scenario", "scenario": scenario.id, "status": "skipped", "reason": reason }))?;
            println!("  skipped: {reason}");
            out.skipped = Some(reason);
            return Ok(());
        }
        let shown = netem::apply(shaping, &s.receiver, wanted)
            .await
            .context("apply netem")?;
        s.record(&json!({ "kind": "scenario", "scenario": scenario.id, "status": "shaped", "netem": shown }))?;
        out.netem = Some(shown);
    }
    let measured = measure(s, scenario, out).await;
    // Undone here as well as at the end of the run, so the next scenario starts on a clean link.
    let undone = shaping.undo().await;
    measured?;
    undone.context("remove netem")
}

/// Why this receiver cannot be shaped, if it cannot.
fn cannot_shape(s: &Session) -> Option<String> {
    if s.receiver.os != Os::Linux {
        return Some(format!(
            "netem needs a Linux receiver, and the receiver is {:?}",
            s.receiver.os
        ));
    }
    found(&s.receiver_tools, "tc")
        .is_none()
        .then(|| "tc not found on the receiver".to_string())
}

async fn measure(s: &mut Session, scenario: &Scenario, out: &mut ScenarioReport) -> Result<()> {
    let baseline = mscp::unavailable(s).is_none();
    let wanted = Wanted::of(scenario, baseline);
    out.ceilings = ceilings::measure(s, scenario, wanted, &mut out.not_measured).await?;
    for workload in &scenario.workloads {
        mscp::measure(s, scenario, workload, &mut out.rows, &mut out.not_measured).await?;
        for config in &scenario.probe {
            let row = probe_run::run_config(s, scenario, workload, config).await?;
            out.rows.push(row);
        }
    }
    Ok(())
}
