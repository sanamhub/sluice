//! A `sluice daemon` process for the tests that call it, with its own home, its roots on
//! loopback, and its log kept in a file.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use sluice_api::Client;
use sluice_api::proto::event::Event as Kind;
use sluice_api::proto::{Event, RootPath, WatchEventsRequest};

use super::EXE;

/// How long a test waits for anything the daemon does, so a hang fails instead of stalling.
pub const PATIENCE: Duration = Duration::from_secs(120);

pub struct Running {
    pub home: PathBuf,
    pub log: PathBuf,
    child: Child,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts a daemon on `home` with `roots`, waits for the line that says it listens, and returns it.
/// `log_level` is what `RUST_LOG` is set to.
pub fn start(home: &Path, roots: &[&Path], log_level: &str) -> Running {
    let log = home.with_extension("log");
    let mut command = Command::new(EXE);
    command
        .env("SLUICE_HOME", home)
        .env("RUST_LOG", log_level)
        .env_remove("NO_COLOR")
        .args(["daemon", "--loopback"]);
    for root in roots {
        command.arg("--root").arg(root);
    }
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(File::create(&log).unwrap())
        .spawn()
        .expect("start the daemon");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .expect("read the listening line");
    assert!(
        line.starts_with("listening on "),
        "the daemon did not start: {line:?}\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    Running {
        home: home.to_path_buf(),
        log,
        child,
    }
}

impl Running {
    /// A client of this daemon.
    pub async fn client(&self) -> Client {
        sluice_api::connect(&self.home).await.expect("connect")
    }

    /// The process id, for a test that sends a signal.
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Waits for the daemon process to end and returns whether it ended well.
    pub fn wait_for_exit(&mut self) -> bool {
        self.child.wait().expect("wait").success()
    }

    /// Everything the daemon has written to its log so far.
    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

/// A path under the root `root`, as a request names it.
pub fn at(root: &Path, path: &str) -> Option<RootPath> {
    Some(RootPath {
        root: root.to_str().unwrap().to_string(),
        path: path.to_string(),
    })
}

/// Every event of a job until it ends, read from the daemon's stream.
pub async fn events_of(client: &mut Client, job_id: &str) -> Vec<Event> {
    let mut stream = client
        .watch_events(WatchEventsRequest {
            job_id: job_id.to_string(),
        })
        .await
        .expect("watch")
        .into_inner();
    let mut events = Vec::new();
    let read = async {
        while let Some(next) = stream.message().await.expect("a message") {
            events.extend(next.event);
        }
    };
    tokio::time::timeout(PATIENCE, read)
        .await
        .expect("the job ended in time");
    events
}

/// The state a job ended in.
pub fn ended_in(events: &[Event]) -> i32 {
    events
        .iter()
        .rev()
        .find_map(|event| match &event.event {
            Some(Kind::Done(done)) => Some(done.state),
            _ => None,
        })
        .expect("the stream ends with a done event")
}
