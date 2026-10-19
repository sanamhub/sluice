//! `bench smoke` end to end: the harness drives the real probe on this machine and writes its
//! results. It uses the probe only, so it runs on hosts without iperf3, fio or mscp.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

/// The suite is allowed 60 seconds for this test.
const DEADLINE: Duration = Duration::from_secs(60);

fn probe() -> PathBuf {
    // Cargo builds the probe into the same directory when the whole workspace is built.
    let probe = Path::new(env!("CARGO_BIN_EXE_bench"))
        .with_file_name(format!("probe{}", std::env::consts::EXE_SUFFIX));
    assert!(
        probe.is_file(),
        "no probe at {}. Run the workspace's tests, or build it with `cargo build -p sluice-probe`.",
        probe.display()
    );
    probe
}

/// Runs `bench smoke` and kills it if it outlives the deadline.
fn smoke(out: &Path) -> (bool, Duration, String) {
    let started = Instant::now();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bench"))
        .args(["smoke", "--probe"])
        .arg(probe())
        .arg("--out")
        .arg(out)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start bench smoke");
    loop {
        if child.try_wait().expect("poll bench").is_some() {
            break;
        }
        if started.elapsed() > DEADLINE {
            let _ = child.kill();
            panic!("bench smoke did not finish within {DEADLINE:?}");
        }
        thread::sleep(Duration::from_millis(100));
    }
    let elapsed = started.elapsed();
    let output = child.wait_with_output().expect("collect output");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    (output.status.success(), elapsed, text)
}

fn lines(out: &Path) -> Vec<Value> {
    std::fs::read_to_string(out.join("raw.jsonl"))
        .expect("raw.jsonl")
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{e}: {l:?}")))
        .collect()
}

#[test]
fn smoke_drives_the_probe_and_writes_results() {
    let out = std::env::temp_dir().join(format!("sluice-bench-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let (success, elapsed, text) = smoke(&out);
    assert!(success, "bench smoke failed:\n{text}");
    assert!(elapsed < DEADLINE);

    let lines = lines(&out);
    let probes: Vec<&Value> = lines.iter().filter(|l| l["kind"] == "probe").collect();
    // 3 configurations, 2 runs each.
    assert_eq!(probes.len(), 6, "{text}");
    for line in &probes {
        assert_eq!(line["status"], "ok", "{line}");
        assert_eq!(line["scenario"], "SMOKE");
        assert!(line["run"].as_u64().is_some_and(|n| n >= 1));
        assert!(line["config"]["label"].is_string());
        // Every fetch connection found its serve line.
        let joined = line["joined"].as_array().expect("joined");
        assert!(!joined.is_empty());
        assert!(joined.iter().all(|j| j["serve_line"].is_u64()), "{line}");
        assert!(
            line["fetch"]["throughput_gbit_s"]
                .as_f64()
                .is_some_and(|g| g > 0.0)
        );
    }

    // The tools a smoke run does not use are on record, as missing or as skipped.
    for kind in ["iperf3", "fio", "mscp"] {
        let line = lines
            .iter()
            .find(|l| l["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} line"));
        assert!(
            matches!(line["status"].as_str(), Some("missing" | "skipped")),
            "{line}"
        );
        assert!(line["reason"].as_str().is_some_and(|r| !r.is_empty()));
    }

    let env: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("env.json")).expect("env.json"))
            .expect("env.json is JSON");
    assert_eq!(env["hosts"].as_array().map(Vec::len), Some(2));
    assert!(env["commands"].as_array().is_some_and(|c| !c.is_empty()));

    let summary = std::fs::read_to_string(out.join("summary.md")).expect("summary.md");
    assert!(summary.contains("## SMOKE"), "{summary}");
    assert!(
        summary.contains("| workload | configuration |"),
        "{summary}"
    );

    let _ = std::fs::remove_dir_all(&out);
}
