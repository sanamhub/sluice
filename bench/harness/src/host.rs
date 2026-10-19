//! Running commands on a host: directly for `local`, through the system `ssh` binary otherwise.

use std::{
    process::Stdio,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use tokio::{process::Command, time::timeout};

use crate::hosts::Entry;

mod spawn;

pub use spawn::Spawned;
use spawn::{Remote, kill_remote};

/// Directories that hold `tc`, `ip` and package-manager installs but are missing from the PATH of
/// a non-interactive ssh session.
const EXTRA_PATH: &str = "/usr/sbin:/sbin:/usr/local/bin:/opt/homebrew/bin";

/// What the harness knows about a host's operating system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows. Only `local` can be Windows.
    Windows,
    /// Anything else `uname` names.
    Other,
}

/// Where a command runs.
#[derive(Clone, Debug)]
enum Target {
    Local,
    Ssh(String),
}

/// One command the harness ran, for `env.json`.
#[derive(Clone, Debug, Serialize)]
pub struct Logged {
    /// `sender` or `receiver`.
    pub host: &'static str,
    /// The command as a person would type it.
    pub command: String,
}

/// What every host shares: the command log and the remote processes still alive.
#[derive(Clone, Default)]
pub struct Ctx {
    commands: Arc<Mutex<Vec<Logged>>>,
    remote: Arc<Mutex<Vec<Remote>>>,
    counter: Arc<AtomicU64>,
}

impl Ctx {
    /// Every command run so far.
    pub fn commands(&self) -> Vec<Logged> {
        self.commands.lock().expect("command log").clone()
    }

    /// Kills the remote processes that are still registered. Runs after an error or ctrl-c,
    /// because killing the local ssh client does not stop what it started.
    pub async fn reap_remote(&self) {
        let left: Vec<Remote> = std::mem::take(&mut *self.remote.lock().expect("remote list"));
        for remote in left {
            kill_remote(&remote).await;
        }
    }
}

/// A command that outlived its limit and was killed.
#[derive(Debug)]
pub struct TimedOut(pub String);

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out: {}", self.0)
    }
}

impl std::error::Error for TimedOut {}

/// Finished command output.
#[derive(Debug)]
pub struct Output {
    /// Exit code, or `None` if a signal ended the process.
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

impl Output {
    /// True if the command exited with status 0.
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// One end of the path.
#[derive(Clone)]
pub struct Host {
    /// `sender` or `receiver`.
    pub role: &'static str,
    /// What the hosts file says about it.
    pub entry: Entry,
    /// Its operating system, read when the host was connected.
    pub os: Os,
    target: Target,
    ctx: Ctx,
}

impl Host {
    /// Reaches the host and reads its operating system.
    pub async fn connect(role: &'static str, entry: &Entry, ctx: &Ctx) -> Result<Self> {
        let target = if entry.host == "local" {
            Target::Local
        } else {
            Target::Ssh(entry.host.clone())
        };
        let mut host = Self {
            role,
            entry: entry.clone(),
            os: Os::Other,
            target,
            ctx: ctx.clone(),
        };
        host.os = match host.target {
            Target::Local => match std::env::consts::OS {
                "linux" => Os::Linux,
                "macos" => Os::Macos,
                "windows" => Os::Windows,
                _ => Os::Other,
            },
            Target::Ssh(_) => {
                let out = host
                    .run(&["uname", "-s"], Duration::from_secs(30))
                    .await
                    .with_context(|| format!("reach {} ({})", role, entry.host))?;
                if !out.success() {
                    bail!(
                        "{} ({}): uname failed: {}",
                        role,
                        entry.host,
                        out.stderr.trim()
                    );
                }
                match out.stdout.trim() {
                    "Linux" => Os::Linux,
                    "Darwin" => Os::Macos,
                    _ => Os::Other,
                }
            }
        };
        Ok(host)
    }

    /// A local host with a known OS, for tests that only build command lines.
    #[cfg(test)]
    pub fn for_tests(role: &'static str, entry: Entry, os: Os, ctx: Ctx) -> Self {
        Self {
            role,
            entry,
            os,
            target: Target::Local,
            ctx,
        }
    }

    /// True if commands run on this machine.
    pub fn is_local(&self) -> bool {
        matches!(self.target, Target::Local)
    }

    /// The ssh destination, for commands that start from this host and reach another.
    pub fn destination(&self) -> Option<&str> {
        match &self.target {
            Target::Local => None,
            Target::Ssh(destination) => Some(destination),
        }
    }

    /// Joins a file name to the data directory.
    pub fn data_path(&self, name: &str) -> String {
        let dir = self.entry.data_dir.trim_end_matches(['/', '\\']);
        let separator = if self.os == Os::Windows { '\\' } else { '/' };
        format!("{dir}{separator}{name}")
    }

    fn log(&self, argv: &[String]) {
        let line = Logged {
            host: self.role,
            command: render(argv),
        };
        self.ctx.commands.lock().expect("command log").push(line);
    }

    fn build(&self, argv: &[String], pid_file: Option<&str>) -> Command {
        let mut command = match &self.target {
            Target::Local => {
                let mut command = Command::new(&argv[0]);
                command.args(&argv[1..]);
                if cfg!(unix) {
                    let path = std::env::var("PATH").unwrap_or_default();
                    command.env("PATH", format!("{path}:{EXTRA_PATH}"));
                }
                command
            }
            Target::Ssh(destination) => {
                let record =
                    pid_file.map_or(String::new(), |f| format!("echo $$ > {}; ", quote(f)));
                let script = format!("PATH=\"$PATH:{EXTRA_PATH}\"; {record}exec {}", render(argv));
                let mut command = Command::new("ssh");
                command.args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=15"]);
                command.args(["-o", "ServerAliveInterval=15", "--"]);
                command.arg(destination).arg(script);
                command
            }
        };
        command.stdin(Stdio::null()).kill_on_drop(true);
        command
    }

    /// Runs a command to the end and returns its output. The command is killed after `limit`.
    pub async fn run(&self, argv: &[&str], limit: Duration) -> Result<Output> {
        let argv: Vec<String> = argv.iter().map(|a| (*a).to_string()).collect();
        self.log(&argv);
        let mut command = self.build(&argv, None);
        let finished = timeout(limit, command.output())
            .await
            .map_err(|_| TimedOut(format!("{} after {limit:?}", render(&argv))))?
            .with_context(|| format!("start {}", render(&argv)))?;
        Ok(Output {
            code: finished.status.code(),
            stdout: String::from_utf8_lossy(&finished.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&finished.stderr).into_owned(),
        })
    }

    /// Removes a file the harness made. A file that is already gone is fine.
    pub async fn remove_file(&self, path: &str) -> Result<()> {
        self.remove(path, false).await
    }

    /// Removes a directory the harness made, and everything in it.
    pub async fn remove_dir_all(&self, path: &str) -> Result<()> {
        self.remove(path, true).await
    }

    async fn remove(&self, path: &str, directory: bool) -> Result<()> {
        let flag = if directory { "-rf" } else { "-f" };
        if !self.is_local() {
            let out = self
                .run(&["rm", flag, "--", path], Duration::from_secs(300))
                .await?;
            ensure!(out.success(), "rm {path}: {}", out.stderr.trim());
            return Ok(());
        }
        self.log(&["rm".to_string(), flag.to_string(), path.to_string()]);
        let target = path.to_string();
        let removed = tokio::task::spawn_blocking(move || {
            if directory {
                std::fs::remove_dir_all(&target)
            } else {
                std::fs::remove_file(&target)
            }
        })
        .await
        .context("remove task")?;
        match removed {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other.with_context(|| format!("remove {path}")),
        }
    }
}

/// One argument, quoted for a POSIX shell unless it is plainly safe.
pub fn quote(arg: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// A command line as a person would type it.
pub fn render(argv: &[String]) -> String {
    argv.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_stay_bare_and_the_rest_is_quoted() {
        assert_eq!(quote("--size"), "--size");
        assert_eq!(quote("/data/bench/x.bin"), "/data/bench/x.bin");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote("$(reboot)"), "'$(reboot)'");
    }

    #[test]
    fn a_command_renders_as_one_line() {
        let argv = ["probe", "fetch", "ticket with space"].map(String::from);
        assert_eq!(render(&argv), "probe fetch 'ticket with space'");
    }

    #[test]
    fn data_paths_use_the_hosts_separator() {
        let entry = Entry {
            host: "local".to_string(),
            interface: None,
            address: None,
            probe: "probe".to_string(),
            data_dir: "/data/bench/".to_string(),
        };
        let host = Host {
            role: "sender",
            entry,
            os: Os::Linux,
            target: Target::Local,
            ctx: Ctx::default(),
        };
        assert_eq!(host.data_path("x.bin"), "/data/bench/x.bin");
    }
}
