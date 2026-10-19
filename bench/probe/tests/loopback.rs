//! Runs `probe serve` and `probe fetch` as real processes over the local network and checks the
//! contract the T3 harness relies on.

use std::{
    io::{BufRead, BufReader},
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

fn spawn_server(mode: &str, congestion: &str) -> (Server, ChildStdout) {
    let mut child = probe()
        .args([
            "serve",
            "--size",
            "8MiB",
            "--mode",
            mode,
            "--congestion",
            congestion,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("start probe serve");
    let stdout = child.stdout.take().expect("piped stdout");
    (Server(child), stdout)
}

fn run_case(server_stdout: ChildStdout, mode: &str, congestion: &str, connections: u16) {
    let mut ticket = String::new();
    BufReader::new(server_stdout)
        .read_line(&mut ticket)
        .expect("read the ticket line");
    let ticket = ticket.trim();
    assert!(!ticket.is_empty(), "serve exited before printing a ticket");

    let output = probe()
        .args(["fetch", ticket, "--mode", mode, "--congestion", congestion])
        .args(["--connections", &connections.to_string()])
        .output()
        .expect("run probe fetch");
    assert!(
        output.status.success(),
        "fetch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout must be one line, got {stdout:?}"
    );
    let result: Value = serde_json::from_str(stdout.trim()).expect("stdout is JSON");
    check(&result, mode, congestion, connections);
}

fn check(result: &Value, mode: &str, congestion: &str, connections: u16) {
    assert_eq!(result["mode"], mode);
    assert_eq!(result["size_bytes"], SIZE);
    assert_eq!(result["received_bytes"], SIZE);
    assert_eq!(result["connections"], connections);
    assert_eq!(result["congestion"], congestion);
    assert_eq!(result["window"]["mode"], "default");
    assert_eq!(result["key_exchange_group"], "X25519MLKEM768");
    assert_eq!(result["path"], "direct");
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

fn with_deadline(mode: &'static str, congestion: &'static str, connections: u16) {
    // The guard stays on this thread so it kills the server on a panic or a timeout alike.
    let (_server, stdout) = spawn_server(mode, congestion);
    let (done, finished) = mpsc::channel();
    let worker = thread::spawn(move || {
        run_case(stdout, mode, congestion, connections);
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
    with_deadline("raw", "bbr3", 1);
}

#[test]
fn raw_bbr3_four_connections() {
    with_deadline("raw", "bbr3", 4);
}

#[test]
fn raw_cubic_one_connection() {
    with_deadline("raw", "cubic", 1);
}

#[test]
fn raw_cubic_four_connections() {
    with_deadline("raw", "cubic", 4);
}

#[test]
fn blobs_bbr3_one_connection() {
    with_deadline("blobs", "bbr3", 1);
}

#[test]
fn blobs_bbr3_four_connections() {
    with_deadline("blobs", "bbr3", 4);
}

#[test]
fn blobs_cubic_one_connection() {
    with_deadline("blobs", "cubic", 1);
}

#[test]
fn blobs_cubic_four_connections() {
    with_deadline("blobs", "cubic", 4);
}
