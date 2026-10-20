//! `serve` reports each connection when it closes, and the line can be joined to the `fetch` result.

use std::collections::BTreeSet;

use serde_json::Value;

mod common;
use common::{log_paths, probe, read_line, read_ticket, spawn_server, within_deadline};

const CONNECTIONS: usize = 2;

fn keys(values: impl Iterator<Item = Value>) -> BTreeSet<String> {
    values
        .map(|v| v["key"].as_str().expect("key is a string").to_string())
        .collect()
}

fn serve_lines(congestion: &'static str) {
    let (_server, mut stdout) = spawn_server("raw", congestion, "8MiB", &[]);
    within_deadline(move |fetches| {
        let ticket = read_ticket(&mut stdout);
        let fetched = fetches.run(
            probe()
                .args([
                    "fetch",
                    &ticket,
                    "--mode",
                    "raw",
                    "--congestion",
                    congestion,
                ])
                .args(["--connections", &CONNECTIONS.to_string()]),
        );
        assert!(fetched.status.success(), "fetch failed");
        let result: Value = serde_json::from_slice(&fetched.stdout).expect("fetch prints JSON");
        let received = result["connection_stats"]
            .as_array()
            .expect("array")
            .clone();

        // `serve` prints each line after the client closes, which `fetch` does as it exits.
        let lines: Vec<Value> = (0..CONNECTIONS)
            .map(|_| {
                let line = read_line(&mut stdout);
                serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line:?}"))
            })
            .collect();

        assert_eq!(keys(lines.iter().cloned()), keys(received.iter().cloned()));
        received.iter().for_each(|entry| log_paths("fetch", entry));
        assert_eq!(keys(lines.iter().cloned()).len(), CONNECTIONS);
        for line in &lines {
            log_paths("serve", line);
            assert_eq!(line["event"], "connection_closed");
            assert!(line["close_reason"].as_str().is_some_and(|r| !r.is_empty()));
            assert!(line["received_bytes"].is_null());
            // Read with getrusage, which Windows has no safe call for.
            assert_eq!(line["cpu_user_s"].is_f64(), cfg!(unix), "{line}");
            assert_eq!(line["cpu_sys_s"].is_f64(), cfg!(unix), "{line}");
            assert!(line["sent_datagrams"].as_u64().is_some_and(|n| n > 0));
            // The sender is the other end of the fetch, so the two never name the same peer.
            assert_ne!(line["remote_id"], received[0]["remote_id"]);
            let path = &line["paths"][0];
            assert!(path["cwnd_bytes"].as_u64().is_some_and(|n| n > 0), "{line}");
            let controller = &path["controller"];
            assert!(controller["pacing_rate_bytes_s"].is_u64() == (congestion == "bbr3"));
            assert_eq!(controller["bbr_state"].is_string(), congestion == "bbr3");
            assert_eq!(controller["bytes_in_flight"].is_u64(), congestion == "bbr3");
        }
    });
}

#[test]
fn serve_prints_one_line_per_closed_connection_with_bbr_state() {
    serve_lines("bbr3");
}

#[test]
fn serve_leaves_bbr_fields_null_under_cubic() {
    serve_lines("cubic");
}

/// One line per sample, so a failed check shows how many paths were listed, how many of them had
/// no controller, and which one was read.
fn describe(samples: &[Value]) -> String {
    samples
        .iter()
        .map(|s| {
            format!(
                "t={} open_paths={} stale_paths={} path={} controller={}",
                s["elapsed_s"],
                s["open_paths"],
                s["stale_paths"],
                s["path"],
                s["cwnd_bytes"].is_u64()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Entries in a `controller_series` array, checked for the shape both ends promise.
fn series(side: &str, entry: &Value) -> Vec<Value> {
    let samples = entry["controller_series"].as_array().expect("an array");
    let mut last = 0.0;
    for sample in samples {
        let at = sample["elapsed_s"].as_f64().expect("elapsed_s");
        assert!(
            at > last,
            "elapsed_s does not increase:\n{}",
            describe(samples)
        );
        last = at;
        // BBR's state is read from the same controller as the window, so they are null together.
        assert_eq!(
            sample["cwnd_bytes"].is_u64(),
            sample["bbr_state"].is_string(),
            "{sample}\n{}",
            describe(samples)
        );
    }
    // iroh's list can hold only paths `noq` has dropped for a whole run. Then every sample says so
    // with `stale_paths`, and having no reading is the truth, so it passes and is counted.
    let all_stale = !samples.is_empty()
        && samples.iter().all(|s| {
            s["open_paths"].as_u64().is_some_and(|n| n > 0) && s["stale_paths"] == s["open_paths"]
        });
    if all_stale {
        println!("STALE side={side} samples={}", samples.len());
    }
    assert!(
        all_stale || samples.iter().any(|s| s["cwnd_bytes"].is_u64()),
        "no sample found a path:\n{}",
        describe(samples)
    );
    samples.clone()
}

/// Seconds `fetch` may transfer. Samples come at 1, 2, 3 and 4 seconds, and at 5 if that tick wins
/// the race with the timeout. The check wants 3, so a machine that is a tick late still passes.
const SERIES_TIMEOUT: &str = "5";

#[test]
fn both_ends_sample_the_controller_once_a_second() {
    // Sized so that no machine finishes in `SERIES_TIMEOUT`, which makes the run take that long
    // everywhere. A size that finishes is a test that takes as long as the machine is slow.
    let (_server, mut stdout) = spawn_server("raw", "bbr3", "64GiB", &[]);
    within_deadline(move |fetches| {
        let ticket = read_ticket(&mut stdout);
        let fetched = fetches.run(probe().args([
            "fetch",
            &ticket,
            "--mode",
            "raw",
            "--congestion",
            "bbr3",
            "--progress-lines",
            "--timeout",
            SERIES_TIMEOUT,
        ]));
        assert!(!fetched.status.success(), "a timeout must exit non-zero");
        // The same run shows the live stream: progress lines first, then the result line, which
        // is the only line that is not a progress event.
        let text = String::from_utf8(fetched.stdout).expect("UTF-8");
        let (live, result_line) = text.trim_end().rsplit_once('\n').expect("progress lines");
        let result: Value = serde_json::from_str(result_line).expect("fetch prints JSON");
        assert_eq!(result["outcome"], "timeout");
        let ticks: Vec<Value> = live
            .lines()
            .map(|l| serde_json::from_str(l).expect("progress line"))
            .collect();
        let progress = result["progress"].as_array().expect("progress");
        assert_eq!(ticks.len(), progress.len(), "one line per second");
        for (i, (tick, total)) in ticks.iter().zip(progress).enumerate() {
            assert_eq!(tick["event"], "progress");
            assert_eq!(tick["t"], i as u64 + 1);
            assert_eq!(
                tick["bytes"], *total,
                "same reading as the result's progress"
            );
        }

        log_paths("fetch", &result["connection_stats"][0]);
        let received = series("fetch", &result["connection_stats"][0]);
        // One tick feeds both, so the two arrays are the same length.
        assert_eq!(received.len(), progress.len());
        assert!(received.len() >= 3, "{}", describe(&received));

        // `fetch` closes the connection as it exits, which is when `serve` prints this line.
        let line = read_line(&mut stdout);
        let closed: Value = serde_json::from_str(&line).expect("serve prints JSON");
        log_paths("serve", &closed);
        let sent = series("serve", &closed);
        assert!(!sent.is_empty());
    });
}
