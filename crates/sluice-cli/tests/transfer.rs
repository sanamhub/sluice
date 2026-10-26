//! Two real `sluice` processes on loopback: the sender prints a ticket and the receiver writes the
//! file. No relay and no internet are involved.

use std::{
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const EXE: &str = env!("CARGO_BIN_EXE_sluice");

struct Sender(Child);

impl Drop for Sender {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(home: &Path, file: &Path, expires: &str) -> (Sender, String) {
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", home)
        .args(["send", file.to_str().unwrap(), "--expires", expires])
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

fn recv(home: &Path, ticket: &str, out: &Path, force: bool) -> std::process::Output {
    let mut command = Command::new(EXE);
    command
        .env("SLUICE_HOME", home)
        .args(["recv", ticket, "--out", out.to_str().unwrap()]);
    if force {
        command.arg("--force");
    }
    command.output().expect("run recv")
}

fn wait_for_sender(sender: &mut Sender) {
    let deadline = Instant::now() + Duration::from_secs(62);
    loop {
        if sender.0.try_wait().unwrap().is_some() {
            return;
        }
        assert!(Instant::now() < deadline, "the sender did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn data(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

#[test]
fn one_file_crosses_two_processes_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("notes.bin");
    let body = data(300 * 1024, 7);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let result = recv(&home, &ticket, &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(out.join("notes.bin")).unwrap(), body);
    wait_for_sender(&mut sender);
}

#[test]
fn a_name_clash_refuses_before_writing_and_force_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let file = dir.path().join("a.txt");
    std::fs::write(&file, b"new body").unwrap();
    std::fs::write(out.join("a.txt"), b"old body").unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"old body");

    let replaced = recv(&home, &ticket, &out, true);
    assert!(
        replaced.status.success(),
        "{}",
        String::from_utf8_lossy(&replaced.stderr)
    );
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"new body");
    wait_for_sender(&mut sender);
}

#[test]
fn an_expired_ticket_is_refused_and_the_sender_stops() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("short.txt");
    std::fs::write(&file, b"short lived").unwrap();

    let (mut sender, ticket) = start(&home, &file, "1s");
    std::thread::sleep(Duration::from_millis(1200));
    let refused = recv(&home, &ticket, &out, false);
    assert_eq!(refused.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("expired"));
    wait_for_sender(&mut sender);
}

#[test]
fn the_sender_refuses_a_second_send_from_one_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let first = dir.path().join("first.bin");
    let second = dir.path().join("second.bin");
    std::fs::write(&first, data(1024, 1)).unwrap();
    std::fs::write(&second, data(1024, 2)).unwrap();
    let (_sender, _ticket) = start(&home, &first, "1h");
    let refused = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["send", second.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(6));
    let text = String::from_utf8_lossy(&refused.stderr);
    assert!(text.contains("send both in one command instead"), "{text}");
}
