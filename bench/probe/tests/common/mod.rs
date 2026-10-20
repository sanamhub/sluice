//! Helpers for the tests that run `probe serve` and `probe fetch` as processes.

use std::{
    io::{BufRead, BufReader},
    process::{Child, ChildStdout, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

use serde_json::Value;

/// Well under the 60 seconds the suite is allowed, so a hang fails here with a message instead of
/// being killed by the runner.
pub const DEADLINE: Duration = Duration::from_secs(45);

pub struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        // `serve` runs until killed, and a failing or hung test must not leave it behind.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn probe() -> Command {
    Command::new(env!("CARGO_BIN_EXE_probe"))
}

pub fn spawn_server(
    mode: &str,
    congestion: &str,
    size: &str,
    extra: &[String],
) -> (Server, BufReader<ChildStdout>) {
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
    (Server(child), BufReader::new(stdout))
}

/// The next line `serve` printed, without its newline. Empty if `serve` exited.
pub fn read_line(server_stdout: &mut BufReader<ChildStdout>) -> String {
    let mut line = String::new();
    server_stdout.read_line(&mut line).expect("read a line");
    line.trim().to_string()
}

pub fn read_ticket(server_stdout: &mut BufReader<ChildStdout>) -> String {
    let ticket = read_line(server_stdout);
    assert!(!ticket.is_empty(), "serve exited before printing a ticket");
    ticket
}

/// With `PROBE_TEST_LOG_PATHS` set, prints where a connection's `paths` came from and how its
/// samples chose a path, so a loop of the suite can count how often each was used. Off by default.
pub fn log_paths(side: &str, entry: &Value) {
    if std::env::var_os("PROBE_TEST_LOG_PATHS").is_none() {
        return;
    }
    println!(
        "PATHS side={side} source={} paths_at_s={} paths={}",
        entry["paths_source"],
        entry["paths_at_s"],
        entry["paths"].as_array().map_or(0, Vec::len),
    );
    let samples = entry["controller_series"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    if !samples.is_empty() {
        let count = |path: &str| samples.iter().filter(|s| s["path"] == path).count();
        let none = samples.iter().filter(|s| s["path"].is_null()).count();
        println!(
            "SAMPLES side={side} selected={} only={} busiest={} none={none}",
            count("selected"),
            count("only"),
            count("busiest"),
        );
    }
}

pub fn within_deadline(case: impl FnOnce() + Send + 'static) {
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
