//! Helpers for the tests that run `probe serve` and `probe fetch` as processes.

// Each test binary uses its own part of these helpers.
#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, ChildStderr, ChildStdout, Command, Output, Stdio},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        mpsc::{self, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
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
    let mut command = probe();
    if logging() && std::env::var_os("RUST_LOG").is_none() {
        // `serve` has no `fetch` result to carry its path readings, so its debug lines are how a
        // loop sees which reading a `paths` list came from. A `RUST_LOG` set by hand wins, to add
        // iroh's own targets.
        command.env("RUST_LOG", "warn,probe::paths=debug");
    }
    let mut child = command
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

fn logging() -> bool {
    std::env::var_os("PROBE_TEST_LOG_PATHS").is_some()
}

/// With `PROBE_TEST_LOG_PATHS` set, prints where a connection's `paths` came from and how its
/// samples chose a path, so a loop of the suite can count how often each was used. Off by default.
pub fn log_paths(side: &str, entry: &Value) {
    if !logging() {
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
        let stale = samples
            .iter()
            .filter(|s| s["stale_paths"].as_u64().is_some_and(|n| n > 0))
            .count();
        println!(
            "SAMPLES side={side} selected={} only={} busiest={} none={none} stale={stale}",
            count("selected"),
            count("only"),
            count("busiest"),
        );
    }
}

/// Runs `case` on a worker thread and fails if it has not finished within `DEADLINE`. Every fetch
/// it starts is killed and reaped when this returns or panics, including after the deadline.
pub fn within_deadline(case: impl FnOnce(&Fetches) + Send + 'static) {
    within_deadline_of(DEADLINE, &Arc::default(), case);
}

/// `within_deadline` with the deadline and the fetches given, so a test can shorten the one and
/// look at the other afterwards.
pub fn within_deadline_of(
    deadline: Duration,
    fetches: &Arc<Fetches>,
    case: impl FnOnce(&Fetches) + Send + 'static,
) {
    // On this thread, so it runs when the wait below gives up and the worker is still blocked.
    let _reaper = Reaper(Arc::clone(fetches));
    let (done, finished) = mpsc::channel();
    let shared = Arc::clone(fetches);
    let worker = thread::spawn(move || {
        case(&shared);
        let _ = done.send(());
    });
    match finished.recv_timeout(deadline) {
        Ok(()) => worker.join().expect("worker panicked"),
        // The worker panicked before sending, which drops the sender.
        Err(RecvTimeoutError::Disconnected) => {
            std::panic::resume_unwind(worker.join().expect_err("worker ended without a result"))
        }
        Err(RecvTimeoutError::Timeout) => panic!("no result within {deadline:?}"),
    }
}

/// The `probe fetch` processes a test has started.
#[derive(Default)]
pub struct Fetches(Mutex<Vec<Arc<Started>>>);

struct Started {
    child: Mutex<Child>,
    /// What `fetch` has written to stderr so far, filled as it arrives so a hung run still has it.
    stderr: Mutex<String>,
    /// Taken by whoever joins it first, the worker or the reaper.
    reader: Mutex<Option<JoinHandle<()>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking test is the normal way for this to be poisoned, and the data is still good.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Started {
    fn read_stderr(&self, stderr: ChildStderr) {
        let mut reader = BufReader::new(stderr);
        let mut raw = Vec::new();
        while reader.read_until(b'\n', &mut raw).is_ok_and(|n| n > 0) {
            let line = String::from_utf8_lossy(&raw).into_owned();
            if logging() {
                println!("STDERR side=fetch {}", line.trim_end());
            }
            lock(&self.stderr).push_str(&line);
            raw.clear();
        }
    }

    fn join_reader(&self) {
        if let Some(reader) = lock(&self.reader).take() {
            let _ = reader.join();
        }
    }
}

impl Fetches {
    /// Runs a `probe fetch` to the end. Its stderr is read as it arrives. With
    /// `PROBE_TEST_LOG_PATHS` set each line is printed at once, to be counted by a loop. Without
    /// it the lines are printed only if the test fails, which is when `Reaper` runs on a panic.
    pub fn run(&self, cmd: &mut Command) -> Output {
        let mut child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run probe fetch");
        let mut stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let started = Arc::new(Started {
            child: Mutex::new(child),
            stderr: Mutex::default(),
            reader: Mutex::default(),
        });
        let reader = Arc::clone(&started);
        *lock(&started.reader) = Some(thread::spawn(move || reader.read_stderr(stderr)));
        lock(&self.0).push(Arc::clone(&started));

        // Both pipes end when the process does, whether it exits or the reaper kills it.
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).expect("read fetch's stdout");
        started.join_reader();
        let status = lock(&started.child).wait().expect("wait for probe fetch");
        let stderr = lock(&started.stderr).clone().into_bytes();
        Output {
            status,
            stdout: out,
            stderr,
        }
    }

    /// True when every fetch started so far has been waited for.
    pub fn all_reaped(&self) -> bool {
        lock(&self.0).iter().all(|s| {
            lock(&s.child)
                .try_wait()
                .is_ok_and(|status| status.is_some())
        })
    }
}

/// Kills and waits for every fetch when dropped, like `Server` does for `serve`. A fetch whose
/// worker gave up at the deadline would otherwise run on after the test, up to its own `--timeout`.
struct Reaper(Arc<Fetches>);

impl Drop for Reaper {
    fn drop(&mut self) {
        let failing = thread::panicking();
        for started in lock(&self.0.0).iter() {
            {
                let mut child = lock(&started.child);
                let _ = child.kill();
                let _ = child.wait();
            }
            started.join_reader();
            if failing && !logging() {
                for line in lock(&started.stderr).lines() {
                    println!("STDERR side=fetch {line}");
                }
            }
        }
    }
}
