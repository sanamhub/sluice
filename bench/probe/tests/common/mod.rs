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

/// A running `probe serve`. Shared with the `Probes` it was started through, if any, so that either
/// can kill it.
pub struct Server(Arc<Mutex<Child>>);

impl Drop for Server {
    fn drop(&mut self) {
        // `serve` runs until killed, and a failing or hung test must not leave it behind.
        kill_and_wait(&self.0);
    }
}

fn kill_and_wait(child: &Mutex<Child>) {
    let mut child = lock(child);
    let _ = child.kill();
    let _ = child.wait();
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
    (Server(Arc::new(Mutex::new(child))), BufReader::new(stdout))
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

/// Checks the paths of one `serve` line or `fetch` entry and returns the path to read, as the
/// samples choose it among the listed paths that have a controller: the selected one, else the one
/// that sent the most datagrams. `None` when iroh listed no such path, which passes only if
/// `unlisted_path_ids` shows `noq` held one iroh did not list. That is iroh's fault and not the
/// probe's, in either of its two shapes (an empty list, or a list of paths `noq` no longer holds).
/// An `UNLISTED` line is printed whenever the ids are not empty, so a loop can count both.
pub fn path_in_use<'a>(side: &str, entry: &'a Value) -> Option<&'a Value> {
    let listed = entry["paths"].as_array().expect("paths is an array");
    let unlisted = entry["unlisted_path_ids"]
        .as_array()
        .unwrap_or_else(|| panic!("no unlisted_path_ids: {entry}"));
    if !unlisted.is_empty() {
        println!(
            "UNLISTED side={side} ids={} listed={}",
            entry["unlisted_path_ids"],
            listed.len()
        );
    }
    let with_controller = || listed.iter().filter(|p| !p["controller"].is_null());
    let chosen = with_controller()
        .find(|p| p["selected"] == true)
        .or_else(|| with_controller().max_by_key(|p| p["sent_datagrams"].as_u64()));
    if chosen.is_none() {
        assert!(
            !unlisted.is_empty(),
            "no listed path has a controller and noq holds none iroh did not list: {entry}"
        );
    }
    if listed.is_empty() {
        assert_eq!(entry["paths_source"], "none", "{entry}");
    }
    chosen
}

/// Runs `case` on a worker thread and fails if it has not finished within `DEADLINE`. Every fetch
/// and serve it starts through `Probes` is killed and reaped when this returns or panics,
/// including after the deadline.
pub fn within_deadline(case: impl FnOnce(&Probes) + Send + 'static) {
    within_deadline_of(DEADLINE, &Arc::default(), case);
}

/// `within_deadline` with the deadline and the processes given, so a test can shorten the one and
/// look at the other afterwards.
pub fn within_deadline_of(
    deadline: Duration,
    probes: &Arc<Probes>,
    case: impl FnOnce(&Probes) + Send + 'static,
) {
    // On this thread, so it runs when the wait below gives up and the worker is still blocked.
    let _reaper = Reaper(Arc::clone(probes));
    let (done, finished) = mpsc::channel();
    let shared = Arc::clone(probes);
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

/// The `probe fetch` and `probe serve` processes a test has started inside `within_deadline`.
#[derive(Default)]
pub struct Probes {
    fetches: Mutex<Vec<Arc<Started>>>,
    servers: Mutex<Vec<Arc<Mutex<Child>>>>,
}

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

impl Probes {
    /// Starts a `probe serve` like `spawn_server`, and kills it when the test ends even if the
    /// worker never returns.
    pub fn serve(
        &self,
        mode: &str,
        congestion: &str,
        size: &str,
        extra: &[String],
    ) -> (Server, BufReader<ChildStdout>) {
        let (server, stdout) = spawn_server(mode, congestion, size, extra);
        lock(&self.servers).push(Arc::clone(&server.0));
        (server, stdout)
    }

    /// Runs a `probe fetch` to the end. Its stderr is read as it arrives. With
    /// `PROBE_TEST_LOG_PATHS` set each line is printed at once, to be counted by a loop. Without
    /// it the lines are printed only if the test fails, which is when `Reaper` runs on a panic.
    pub fn fetch(&self, cmd: &mut Command) -> Output {
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
        lock(&self.fetches).push(Arc::clone(&started));

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

    /// True when every fetch and serve started so far has been waited for.
    pub fn all_reaped(&self) -> bool {
        let exited =
            |child: &Mutex<Child>| lock(child).try_wait().is_ok_and(|status| status.is_some());
        lock(&self.fetches).iter().all(|s| exited(&s.child))
            && lock(&self.servers).iter().all(|c| exited(c))
    }
}

/// Kills and waits for every fetch and serve when dropped, as `Server` does for one `serve`. A
/// process whose worker gave up at the deadline would otherwise run on after the test, a fetch up to
/// its own `--timeout` and a serve for ever.
struct Reaper(Arc<Probes>);

impl Drop for Reaper {
    fn drop(&mut self) {
        let failing = thread::panicking();
        for started in lock(&self.0.fetches).iter() {
            kill_and_wait(&started.child);
            started.join_reader();
            if failing && !logging() {
                for line in lock(&started.stderr).lines() {
                    println!("STDERR side=fetch {line}");
                }
            }
        }
        for server in lock(&self.0.servers).iter() {
            kill_and_wait(server);
        }
    }
}
