//! Starting a command on a host and following it: its lines, its stderr, and how to stop it.

use std::{
    collections::VecDeque,
    process::Stdio,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use anyhow::{Context, Result};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

use super::{Ctx, Host, Target, quote, render};

/// Lines of stderr kept for an error message.
const STDERR_LINES: usize = 40;

/// A process started on a remote host, identified by the file that holds its pid.
#[derive(Clone, Debug)]
pub(super) struct Remote {
    destination: String,
    pid_file: String,
}

impl Host {
    /// Starts a command and returns a handle to read its lines and stop it.
    pub fn spawn(&self, argv: &[String]) -> Result<Spawned> {
        self.log(argv);
        let pid_file = match &self.target {
            Target::Local => None,
            Target::Ssh(_) => {
                let n = self.ctx.counter.fetch_add(1, Ordering::Relaxed);
                let name = format!(".bench-{}-{n}.pid", std::process::id());
                Some(self.data_path(&name))
            }
        };
        let mut command = self.build(argv, pid_file.as_deref());
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .with_context(|| format!("start {}", render(argv)))?;
        let (lines, queue) = mpsc::unbounded_channel();
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let stdout = child.stdout.take().context("piped stdout")?;
        let stderr = child.stderr.take().context("piped stderr")?;
        let readers = vec![
            tokio::spawn(forward(stdout, lines)),
            tokio::spawn(keep_tail(stderr, Arc::clone(&tail))),
        ];
        let remote = match (&self.target, pid_file) {
            (Target::Ssh(destination), Some(pid_file)) => {
                let remote = Remote {
                    destination: destination.clone(),
                    pid_file,
                };
                self.ctx
                    .remote
                    .lock()
                    .expect("remote list")
                    .push(remote.clone());
                Some(remote)
            }
            _ => None,
        };
        Ok(Spawned {
            ctx: self.ctx.clone(),
            child,
            queue,
            readers,
            tail,
            remote,
        })
    }
}

async fn forward(reader: impl AsyncRead + Unpin, lines: mpsc::UnboundedSender<String>) {
    let mut reader = BufReader::new(reader).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        if lines.send(line).is_err() {
            break;
        }
    }
}

async fn keep_tail(reader: impl AsyncRead + Unpin, tail: Arc<Mutex<VecDeque<String>>>) {
    let mut reader = BufReader::new(reader).lines();
    while let Ok(Some(line)) = reader.next_line().await {
        let mut tail = tail.lock().expect("stderr tail");
        if tail.len() == STDERR_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
}

pub(super) async fn kill_remote(remote: &Remote) {
    let file = quote(&remote.pid_file);
    let script = format!("kill -KILL \"$(cat {file})\" 2>/dev/null; rm -f {file}");
    let mut command = Command::new("ssh");
    command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "--"]);
    command.arg(&remote.destination).arg(script);
    command.stdin(Stdio::null()).kill_on_drop(true);
    // Best effort: the process may already be gone, and an unreachable host cannot be helped.
    let _ = timeout(Duration::from_secs(30), command.output()).await;
}

/// How a spawned command ended.
#[derive(Debug)]
pub struct Finished {
    /// Exit code, `None` after a signal or a kill.
    pub code: Option<i32>,
    /// True if the harness killed it because it outlived its limit.
    pub killed: bool,
}

/// A running command.
pub struct Spawned {
    ctx: Ctx,
    child: Child,
    queue: mpsc::UnboundedReceiver<String>,
    readers: Vec<JoinHandle<()>>,
    tail: Arc<Mutex<VecDeque<String>>>,
    remote: Option<Remote>,
}

impl Spawned {
    /// The next line of stdout. `None` once the process has closed it, and an error after `limit`.
    pub async fn next_line(&mut self, limit: Duration) -> Result<Option<String>> {
        timeout(limit, self.queue.recv())
            .await
            .map_err(|_| anyhow::anyhow!("no output within {limit:?}"))
    }

    /// Waits up to `limit` for the process to exit, and kills it if it does not.
    pub async fn finish(&mut self, limit: Duration) -> Result<Finished> {
        let waited = timeout(limit, self.child.wait()).await;
        let finished = match waited {
            Ok(status) => Finished {
                code: status.context("wait")?.code(),
                killed: false,
            },
            Err(_) => {
                self.kill().await;
                Finished {
                    code: None,
                    killed: true,
                }
            }
        };
        for reader in self.readers.drain(..) {
            // Both readers end when the pipes close, which they have by now.
            let _ = reader.await;
        }
        self.forget_remote();
        Ok(finished)
    }

    /// Every stdout line still queued. Call after the process has exited.
    pub fn drain(&mut self) -> Vec<String> {
        std::iter::from_fn(|| self.queue.try_recv().ok()).collect()
    }

    /// Kills the process, and the remote one it started if there is one.
    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
        if let Some(remote) = &self.remote {
            kill_remote(remote).await;
        }
        self.forget_remote();
    }

    /// The last lines the process wrote to stderr.
    pub fn stderr_tail(&self) -> String {
        let tail = self.tail.lock().expect("stderr tail");
        tail.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    fn forget_remote(&mut self) {
        if let Some(remote) = self.remote.take() {
            let mut list = self.ctx.remote.lock().expect("remote list");
            list.retain(|r| r.pid_file != remote.pid_file);
        }
    }
}
