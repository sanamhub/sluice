//! The command lines the harness gives `probe`, and how it reads what comes back.

use serde_json::{Value, json};

use crate::{
    host::{Host, Os},
    scenarios::{Endpoints, ProbeConfig, Sink, Store},
    summary::{RunResult, Status},
};

/// How long past `--timeout` a `fetch` may live before the harness kills it.
pub const KILL_AFTER: u64 = 30;

/// The `probe serve` command line.
pub fn serve_args(host: &Host, config: &ProbeConfig, size: &str) -> Vec<String> {
    let mut argv = vec![
        host.entry.probe.clone(),
        "serve".into(),
        "--size".into(),
        size.into(),
    ];
    argv.extend(common(config));
    let endpoints = match config.endpoints {
        Endpoints::Shared => 1,
        Endpoints::Separate => config.connections,
    };
    argv.extend(["--endpoints".to_string(), endpoints.to_string()]);
    if let Some(store) = config.store() {
        argv.extend(["--store".to_string(), word(&store)]);
        if store == Store::Fs {
            argv.extend(["--dir".to_string(), host.entry.data_dir.clone()]);
        }
    }
    argv
}

/// The `probe fetch` command line.
pub fn fetch_args(
    host: &Host,
    config: &ProbeConfig,
    ticket: &str,
    timeout_s: u64,
    sink_dir: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![host.entry.probe.clone(), "fetch".into(), ticket.into()];
    argv.extend(common(config));
    argv.extend(["--connections".to_string(), config.connections.to_string()]);
    argv.extend(["--endpoints".to_string(), word(&config.endpoints)]);
    argv.extend(["--timeout".to_string(), timeout_s.to_string()]);
    if let Some(sink) = config.sink() {
        argv.extend(["--sink".to_string(), word(&sink)]);
        if let (Sink::Store, Some(dir)) = (sink, sink_dir) {
            argv.extend(["--dir".to_string(), dir.to_string()]);
        }
    }
    argv
}

fn common(config: &ProbeConfig) -> Vec<String> {
    vec![
        "--mode".to_string(),
        word(&config.mode),
        "--congestion".to_string(),
        word(&config.congestion),
        "--window".to_string(),
        word(&config.window),
    ]
}

fn word<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}").to_lowercase()
}

/// Pairs each `fetch` connection with the `serve` line that has the same key.
pub fn join(fetch: &Value, serve: &[Value]) -> Vec<Value> {
    let Some(entries) = fetch["connection_stats"].as_array() else {
        return Vec::new();
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let key = entry["key"].as_str();
            let line = key.and_then(|k| serve.iter().position(|l| l["key"].as_str() == Some(k)));
            json!({ "index": index, "key": key, "serve_line": line })
        })
        .collect()
}

/// Decides how the run ended, and refuses a Linux run that did not use GRO.
pub fn classify(fetch: Option<&Value>, killed: bool, receiver: Os) -> (Status, Option<String>) {
    if killed {
        let reason = format!("fetch outlived its timeout by {KILL_AFTER} s and was killed");
        return (Status::Failed, Some(reason));
    }
    let Some(fetch) = fetch else {
        return (
            Status::Failed,
            Some("fetch printed no result line".to_string()),
        );
    };
    let outcome = fetch["outcome"].as_str();
    if !matches!(outcome, Some("ok" | "timeout")) {
        return (Status::Failed, Some(format!("unknown outcome {outcome:?}")));
    }
    let batches = fetch["recv_gro_batches"].as_u64();
    let datagrams = fetch["recv_datagrams"].as_u64().unwrap_or(0);
    // iroh counts the batches the socket received with GRO. None among thousands of datagrams
    // means GRO did not run, and the number would come from a silent fallback.
    if receiver == Os::Linux && batches == Some(0) && datagrams > 0 {
        let reason = format!("GRO did not run: recv_gro_batches is 0 of {datagrams} datagrams");
        return (Status::Failed, Some(reason));
    }
    match outcome {
        Some("ok") => (Status::Ok, None),
        _ => (Status::Timeout, None),
    }
}

/// The result of a run as the summary reads it.
pub fn result_of(status: Status, fetch: Option<&Value>) -> RunResult {
    let progress = fetch
        .and_then(|f| f["progress"].as_array())
        .map(|p| p.iter().filter_map(Value::as_u64).collect());
    RunResult {
        status,
        gbit_s: fetch
            .filter(|_| status == Status::Ok)
            .and_then(|f| f["throughput_gbit_s"].as_f64()),
        progress,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        host::Ctx,
        hosts::Entry,
        scenarios::{Congestion, Mode, Window},
    };

    fn host() -> Host {
        let entry = Entry {
            host: "local".to_string(),
            interface: None,
            address: None,
            probe: "/opt/probe".to_string(),
            data_dir: "/data".to_string(),
        };
        Host::for_tests("sender", entry, Os::Linux, Ctx::default())
    }

    fn blobs() -> ProbeConfig {
        ProbeConfig {
            mode: Mode::Blobs,
            congestion: Congestion::Bbr3,
            window: Window::Tuned,
            connections: 4,
            endpoints: Endpoints::Separate,
            sink: Some(Sink::Store),
            store: Some(Store::Fs),
        }
    }

    #[test]
    fn serve_gets_one_endpoint_per_connection_when_they_are_separate() {
        let line = serve_args(&host(), &blobs(), "10GiB").join(" ");
        assert_eq!(
            line,
            "/opt/probe serve --size 10GiB --mode blobs --congestion bbr3 --window tuned \
             --endpoints 4 --store fs --dir /data"
        );
    }

    #[test]
    fn fetch_writes_to_the_sink_directory_only_for_a_store_sink() {
        let line = fetch_args(&host(), &blobs(), "TICKET", 900, Some("/data/sink")).join(" ");
        assert_eq!(
            line,
            "/opt/probe fetch TICKET --mode blobs --congestion bbr3 --window tuned \
             --connections 4 --endpoints separate --timeout 900 --sink store --dir /data/sink"
        );
        let discard = ProbeConfig {
            sink: Some(Sink::Discard),
            ..blobs()
        };
        let line = fetch_args(&host(), &discard, "T", 60, None).join(" ");
        assert!(line.ends_with("--sink discard"), "{line}");
    }

    #[test]
    fn a_raw_run_has_no_store_and_no_sink() {
        let raw = ProbeConfig {
            mode: Mode::Raw,
            endpoints: Endpoints::Shared,
            connections: 1,
            sink: None,
            store: None,
            ..blobs()
        };
        let serve = serve_args(&host(), &raw, "1GiB").join(" ");
        assert!(serve.ends_with("--endpoints 1"), "{serve}");
        let fetch = fetch_args(&host(), &raw, "T", 60, None).join(" ");
        assert!(!fetch.contains("--sink"), "{fetch}");
    }

    #[test]
    fn serve_lines_join_on_the_key() {
        let fetch =
            json!({ "connection_stats": [{ "key": "aa" }, { "key": "bb" }, { "key": "cc" }] });
        let serve = vec![json!({ "key": "bb" }), json!({ "key": "aa" })];
        let joined = join(&fetch, &serve);
        assert_eq!(joined[0]["serve_line"], 1);
        assert_eq!(joined[1]["serve_line"], 0);
        assert!(joined[2]["serve_line"].is_null());
    }

    #[test]
    fn a_linux_run_without_gro_fails() {
        let ok = json!({ "outcome": "ok", "recv_gro_batches": 0, "recv_datagrams": 700000 });
        let (status, error) = classify(Some(&ok), false, Os::Linux);
        assert_eq!(status, Status::Failed);
        assert!(error.unwrap().contains("GRO did not run"));
        // Other systems do not count GRO batches the same way, so they are not held to it.
        assert_eq!(classify(Some(&ok), false, Os::Macos).0, Status::Ok);
        let with = json!({ "outcome": "ok", "recv_gro_batches": 76000, "recv_datagrams": 756000 });
        assert_eq!(classify(Some(&with), false, Os::Linux).0, Status::Ok);
    }

    #[test]
    fn timeouts_and_dead_runs_are_told_apart() {
        let timeout =
            json!({ "outcome": "timeout", "recv_gro_batches": 10, "recv_datagrams": 100 });
        assert_eq!(
            classify(Some(&timeout), false, Os::Linux).0,
            Status::Timeout
        );
        assert_eq!(classify(None, false, Os::Linux).0, Status::Failed);
        assert_eq!(classify(Some(&timeout), true, Os::Linux).0, Status::Failed);
    }
}
