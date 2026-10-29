//! Helpers for the integration tests. Each test file uses some of them.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command as Process, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::Command;
use assert_cmd::assert::Assert;

/// `sluice` with `SLUICE_HOME` set to `home`, so a test never touches the user's real home.
/// `NO_COLOR` and `RUST_BACKTRACE` are removed, because the developer's shell may set them and
/// both change what is printed.
pub fn sluice(home: &Path) -> Command {
    let mut command = bare();
    command.env("SLUICE_HOME", home);
    command
}

/// `sluice` with no home at all, for commands that never look for one.
pub fn bare() -> Command {
    let mut command = Command::cargo_bin("sluice").unwrap();
    command.env_remove("NO_COLOR").env_remove("RUST_BACKTRACE");
    command
}

pub fn stdout_of(command: &mut Command) -> String {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

pub fn stdout_text(assert: &Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

pub fn stderr_text(assert: &Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

pub fn is_endpoint_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The binary under test, for the tests that run two processes and read a ticket from one.
pub const EXE: &str = env!("CARGO_BIN_EXE_sluice");

pub struct Sender(pub Child);

impl Drop for Sender {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a sender whose current folder is `dir`, so its arguments can be relative.
pub fn start_in(dir: &Path, home: &Path, args: &[&str]) -> (Sender, String) {
    let mut child = Process::new(EXE)
        .env("SLUICE_HOME", home)
        .current_dir(dir)
        .arg("send")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start send");
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .expect("read ticket");
    assert!(line.starts_with("sluice1"), "stdout is a ticket: {line:?}");
    (Sender(child), line.trim().to_string())
}

pub fn start(home: &Path, file: &Path, expires: &str) -> (Sender, String) {
    start_in(
        file.parent().unwrap(),
        home,
        &[
            file.file_name().unwrap().to_str().unwrap(),
            "--expires",
            expires,
        ],
    )
}

/// Runs `recv` with `dir` as its current folder and `args` after the ticket.
pub fn recv_in(dir: &Path, home: &Path, ticket: &str, args: &[&str]) -> std::process::Output {
    Process::new(EXE)
        .env("SLUICE_HOME", home)
        .current_dir(dir)
        .arg("recv")
        .arg(ticket)
        .args(args)
        .output()
        .expect("run recv")
}

pub fn recv(home: &Path, ticket: &str, out: &Path, force: bool) -> std::process::Output {
    let mut args = vec!["--out", out.to_str().unwrap()];
    if force {
        args.push("--force");
    }
    recv_in(out.parent().unwrap_or(out), home, ticket, &args)
}

pub fn recv_range(home: &Path, ticket: &str, out: &Path, range: &str) -> std::process::Output {
    Process::new(EXE)
        .env("SLUICE_HOME", home)
        .args([
            "recv",
            ticket,
            "--out",
            out.to_str().unwrap(),
            "--range",
            range,
        ])
        .output()
        .expect("run recv")
}

pub fn wait_for_sender(sender: &mut Sender) {
    let deadline = Instant::now() + Duration::from_secs(62);
    loop {
        if sender.0.try_wait().unwrap().is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "the sender did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Starts a sender that reads its content from standard input, and returns its ticket.
pub fn start_stdin(home: &Path, body: &[u8], expires: &str) -> (Sender, String) {
    let mut child = Process::new(EXE)
        .env("SLUICE_HOME", home)
        .args(["send", "-", "--expires", expires])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start send");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(body)
        .expect("write standard input");
    drop(child.stdin.take());
    let mut line = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .expect("read ticket");
    assert!(line.starts_with("sluice1"), "stdout is a ticket: {line:?}");
    (Sender(child), line.trim().to_string())
}

/// Waits until a file appears, or fails the test naming it.
pub fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// That the home's private `tmp` holds no store: a command removes the one it made before it ends.
/// The message says whether the lock of a folder that was left can still be taken, which tells a
/// folder no command is removing from one a command still holds.
pub fn assert_no_temporary_store(home: &Path) {
    let mut left = Vec::new();
    for entry in std::fs::read_dir(home.join("tmp"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let path = entry.path();
        let state = match std::fs::File::open(path.join("lock")) {
            Ok(file) if file.try_lock().is_ok() => "free",
            Ok(_) => "held",
            Err(_) => "no lock",
        };
        left.push(format!("{} ({state})", entry.file_name().to_string_lossy()));
    }
    assert!(
        left.is_empty(),
        "a temporary store was left under the home: {left:?}"
    );
}
