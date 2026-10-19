//! Runs `probe serve` and `probe fetch` as real processes over the local network and checks the
//! contract the bench harness relies on.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use serde_json::Value;

const SIZE: u64 = 8 << 20;
/// Well under the 60 seconds the suite is allowed, so a hang fails here with a message instead of
/// being killed by the runner.
const DEADLINE: Duration = Duration::from_secs(45);

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        // `serve` runs until killed, and a failing or hung test must not leave it behind.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn probe() -> Command {
    Command::new(env!("CARGO_BIN_EXE_probe"))
}

/// One loopback run: which options both ends get and what the result line must say about them.
struct Case {
    mode: &'static str,
    congestion: &'static str,
    connections: u16,
    /// Extra arguments for `serve`.
    serve: Vec<String>,
    /// Extra arguments for `fetch`.
    fetch: Vec<String>,
    serve_endpoints: u64,
    fetch_endpoints: u64,
    /// What the result must report as `serve_store`. `None` means `null`.
    serve_store: Option<&'static str>,
}

impl Case {
    fn new(mode: &'static str, congestion: &'static str, connections: u16) -> Self {
        Self {
            mode,
            congestion,
            connections,
            serve: Vec::new(),
            fetch: Vec::new(),
            serve_endpoints: 1,
            fetch_endpoints: 1,
            serve_store: (mode == "blobs").then_some("mem"),
        }
    }
}

/// A directory removed on drop, for the file store.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sluice-probe-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn spawn_server(
    mode: &str,
    congestion: &str,
    size: &str,
    extra: &[String],
) -> (Server, ChildStdout) {
    let mut child = probe()
        .args([
            "serve",
            "--size",
            size,
            "--mode",
            mode,
            "--congestion",
            congestion,
        ])
        .args(extra)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start probe serve");
    let stdout = child.stdout.take().expect("piped stdout");
    (Server(child), stdout)
}

fn read_ticket(server_stdout: ChildStdout) -> String {
    let mut ticket = String::new();
    BufReader::new(server_stdout)
        .read_line(&mut ticket)
        .expect("read the ticket line");
    let ticket = ticket.trim().to_string();
    assert!(!ticket.is_empty(), "serve exited before printing a ticket");
    ticket
}

fn one_json_line(stdout: Vec<u8>) -> Value {
    let stdout = String::from_utf8(stdout).expect("utf-8 stdout");
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout must be one line, got {stdout:?}"
    );
    serde_json::from_str(stdout.trim()).expect("stdout is JSON")
}

fn run_case(server_stdout: ChildStdout, case: &Case) {
    let ticket = read_ticket(server_stdout);
    let output = probe()
        .args([
            "fetch",
            &ticket,
            "--mode",
            case.mode,
            "--congestion",
            case.congestion,
        ])
        .args(["--connections", &case.connections.to_string()])
        .args(&case.fetch)
        .output()
        .expect("run probe fetch");
    assert!(
        output.status.success(),
        "fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    check(&one_json_line(output.stdout), case);
}

fn check(result: &Value, case: &Case) {
    let (mode, connections) = (case.mode, case.connections);
    assert_eq!(result["outcome"], "ok");
    assert_eq!(result["mode"], mode);
    assert_eq!(result["size_bytes"], SIZE);
    assert_eq!(result["received_bytes"], SIZE);
    assert_eq!(result["connections"], connections);
    assert_eq!(result["serve_endpoints"], case.serve_endpoints);
    assert_eq!(result["fetch_endpoints"], case.fetch_endpoints);
    match case.serve_store {
        Some(store) => assert_eq!(result["serve_store"], store),
        None => assert!(result["serve_store"].is_null()),
    }
    assert_eq!(result["congestion"], case.congestion);
    assert_eq!(result["window"]["mode"], "default");
    assert_eq!(result["key_exchange_group"], "X25519MLKEM768");
    assert_eq!(result["path"], "direct");
    assert!(result["progress"].is_array());
    check_connection_stats(result, connections);
    assert!(result["elapsed_s"].as_f64().is_some_and(|s| s > 0.0));
    assert!(
        result["throughput_gbit_s"]
            .as_f64()
            .is_some_and(|g| g > 0.0)
    );
    assert!(result["rtt_ms"].as_f64().is_some_and(|r| r >= 0.0));
    assert!(result["gso_segments"].is_null() && result["gro_segments"].is_null());
    assert!(
        result["iroh_version"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );
    assert!(
        result["iroh_blobs_version"]
            .as_str()
            .is_some_and(|v| !v.is_empty())
    );

    for field in ["cpu_user_s", "cpu_sys_s"] {
        if cfg!(unix) {
            assert!(
                result[field].as_f64().is_some_and(|s| s >= 0.0),
                "{field}: {result}"
            );
        } else {
            assert!(result[field].is_null(), "{field}: {result}");
        }
    }

    if mode == "blobs" {
        let hash = result["verified_hash"]
            .as_str()
            .expect("blobs mode reports the hash");
        assert_eq!(hash.len(), 64);
        assert_eq!(result["sink"], "discard");
    } else {
        assert!(result["verified_hash"].is_null() && result["sink"].is_null());
    }
}

fn check_connection_stats(result: &Value, connections: u16) {
    let stats = result["connection_stats"]
        .as_array()
        .expect("connection_stats is an array");
    assert_eq!(stats.len(), usize::from(connections));
    let received: u64 = stats
        .iter()
        .map(|c| c["received_bytes"].as_u64().expect("received_bytes"))
        .sum();
    assert_eq!(received, result["received_bytes"].as_u64().unwrap());
    for conn in stats {
        assert!(conn["received_datagrams"].as_u64().is_some_and(|n| n > 0));
        assert!(conn["sent_datagrams"].as_u64().is_some_and(|n| n > 0));
        assert!(conn["paths"].as_array().is_some_and(|p| !p.is_empty()));
    }
}

fn with_deadline(case: Case) {
    // The guard stays on this thread so it kills the server on a panic or a timeout alike.
    let (_server, stdout) = spawn_server(case.mode, case.congestion, "8MiB", &case.serve);
    within_deadline(move || run_case(stdout, &case));
}

fn within_deadline(case: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let worker = thread::spawn(move || {
        case();
        let _ = done.send(());
    });
    match finished.recv_timeout(DEADLINE) {
        Ok(()) => worker.join().expect("worker panicked"),
        // The worker panicked before sending, which drops the sender.
        Err(RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(worker.join().expect_err("worker ended without a result"))
        }
        Err(RecvTimeoutError::Timeout) => panic!("no result within {DEADLINE:?}"),
    }
}

#[test]
fn raw_bbr3_one_connection() {
    with_deadline(Case::new("raw", "bbr3", 1));
}

#[test]
fn raw_bbr3_four_connections() {
    with_deadline(Case::new("raw", "bbr3", 4));
}

#[test]
fn raw_cubic_one_connection() {
    with_deadline(Case::new("raw", "cubic", 1));
}

#[test]
fn raw_cubic_four_connections() {
    with_deadline(Case::new("raw", "cubic", 4));
}

#[test]
fn blobs_bbr3_one_connection() {
    with_deadline(Case::new("blobs", "bbr3", 1));
}

#[test]
fn blobs_bbr3_four_connections() {
    with_deadline(Case::new("blobs", "bbr3", 4));
}

#[test]
fn blobs_cubic_one_connection() {
    with_deadline(Case::new("blobs", "cubic", 1));
}

#[test]
fn blobs_cubic_four_connections() {
    with_deadline(Case::new("blobs", "cubic", 4));
}

#[test]
fn blobs_bbr3_four_connections_on_four_separate_endpoints() {
    let mut case = Case::new("blobs", "bbr3", 4);
    case.serve = vec!["--endpoints".into(), "4".into()];
    case.fetch = vec!["--endpoints".into(), "separate".into()];
    case.serve_endpoints = 4;
    case.fetch_endpoints = 4;
    with_deadline(case);
}

#[test]
fn blobs_bbr3_one_connection_from_a_file_store() {
    let dir = TempDir::new("fs-store");
    let mut case = Case::new("blobs", "bbr3", 1);
    case.serve = vec![
        "--store".into(),
        "fs".into(),
        "--dir".into(),
        dir.0.display().to_string(),
    ];
    case.serve_store = Some("fs");
    with_deadline(case);
    // The import references the data file, so the store holds no second copy of it.
    let copies = std::fs::read_dir(dir.0.join("store").join("data"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "data"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(copies, 0, "the store copied the data file");
    assert!(dir.0.join(format!("data-0-{SIZE}.bin")).is_file());
}

#[test]
fn a_timeout_prints_what_was_measured_and_exits_non_zero() {
    // 64 GiB cannot cross loopback in 2 seconds on any machine this runs on.
    let (_server, stdout) = spawn_server("raw", "bbr3", "64GiB", &[]);
    within_deadline(move || {
        let ticket = read_ticket(stdout);
        let output = probe()
            .args(["fetch", &ticket, "--mode", "raw", "--timeout", "2"])
            .output()
            .expect("run probe fetch");
        assert!(!output.status.success(), "a timeout must exit non-zero");
        assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));

        let result = one_json_line(output.stdout);
        assert_eq!(result["outcome"], "timeout");
        let received = result["received_bytes"].as_u64().expect("received_bytes");
        assert!(received > 0 && received < result["size_bytes"].as_u64().unwrap());
        assert!(result["progress"].as_array().is_some_and(|p| !p.is_empty()));
        check_connection_stats(&result, 1);
    });
}
