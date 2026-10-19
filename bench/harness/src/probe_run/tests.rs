//! A run whose transfer collapses is stopped early, and `serve` still reports its connection. The
//! probe is a shell script that behaves like a collapsed one, so the test needs no network.

use std::{os::unix::fs::PermissionsExt, path::Path, time::Instant};

use super::*;
use crate::{
    host::{Ctx, Host, Os},
    hosts::Entry,
    results::Results,
    scenarios,
    session::{Plan, Session},
};

/// `fetch` moves 1000 a second for three seconds and then 1 a second. `serve` prints one
/// connection line when it is interrupted, as the real one does when its connections close.
const FAKE_PROBE: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "probe 0.0.0 (fake)" ;;
  serve)
    trap 'printf "%s\n" "{\"event\":\"connection_closed\",\"key\":\"k1\"}"; exit 0' INT
    echo FAKE-TICKET
    while true; do sleep 1; done ;;
  fetch)
    t=1
    for bytes in 1000 2000 3000 3001 3002 3003 3004 3005 3006 3007 3008 3009; do
      printf '{"event":"progress","t":%s,"bytes":%s}\n' "$t" "$bytes"
      t=$((t + 1))
      sleep 1
    done ;;
esac
"#;

fn host(role: &'static str, probe: &Path, data_dir: &Path) -> Host {
    let entry = Entry {
        host: "local".to_string(),
        interface: None,
        address: None,
        probe: probe.display().to_string(),
        data_dir: data_dir.display().to_string(),
    };
    Host::for_tests(role, entry, Os::Linux, Ctx::default())
}

#[tokio::test]
async fn a_collapsed_transfer_is_stopped_early_and_serve_still_reports_it() {
    let dir = std::env::temp_dir().join(format!("sluice-bench-stall-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    let probe = dir.join("probe");
    std::fs::write(&probe, FAKE_PROBE).expect("write the fake probe");
    std::fs::set_permissions(&probe, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let scenario = scenarios::parse(
        r#"
        [[scenario]]
        id = "T"
        link = "test"
        timeout_s = 60
        stall_s = 3
        workloads = [{ name = "w", size = "1MiB" }]
        probe = [{ mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared" }]
        "#,
    )
    .expect("scenario")
    .remove(0);
    let mut session = Session {
        sender: host("sender", &probe, &dir),
        receiver: host("receiver", &probe, &dir),
        sender_tools: Vec::new(),
        receiver_tools: Vec::new(),
        results: Results::create_in(dir.join("results")).expect("results"),
        runs: 1,
        plan: Plan {
            ceilings_and_baseline: false,
            size_override: None,
        },
        failed: 0,
    };

    let started = Instant::now();
    let row = run_config(
        &mut session,
        &scenario,
        &scenario.workloads[0],
        &scenario.probe[0],
    )
    .await
    .expect("the run is recorded");
    // The script would go on for 12 s. Three quiet seconds after the three busy ones stop it.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );

    assert_eq!(row.runs[0].status, Status::Stalled);
    assert_eq!(
        session.failed, 0,
        "a stalled run is a result, not a failure"
    );
    let progress = row.runs[0].progress.as_deref().expect("live progress");
    assert_eq!(progress.first(), Some(&1000));
    assert!(progress.len() >= 6 && progress.len() < 12, "{progress:?}");

    let raw = std::fs::read_to_string(dir.join("results/raw.jsonl")).expect("raw.jsonl");
    let line: Value = serde_json::from_str(raw.lines().next().expect("a line")).expect("JSON");
    assert_eq!(line["status"], "stalled");
    assert_eq!(line["stalled"], true);
    assert!(line["fetch"].is_null());
    assert_eq!(
        line["progress_lines"].as_array().map(Vec::len),
        Some(progress.len())
    );
    // serve was interrupted, so it printed its connection line.
    assert_eq!(line["serve"][0]["key"], "k1", "{line}");

    let _ = std::fs::remove_dir_all(&dir);
}
