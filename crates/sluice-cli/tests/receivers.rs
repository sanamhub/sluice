//! A sender names each receiver it serves: a short fingerprint, how the connection reached it, and
//! whether the receiver took the whole transfer. A send with places refuses the receiver it cannot
//! serve, once.

mod common;

use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    sync::mpsc,
    time::Duration,
};

use common::{EXE, data, recv};

#[test]
fn a_sender_names_each_receiver_that_connects_and_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("one.bin");
    std::fs::write(&file, data(64 * 1024, 3)).unwrap();
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .current_dir(dir.path())
        .args(["send", "one.bin", "--expires", "1h"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start send");
    let mut ticket = String::new();
    BufReader::new(child.stdout.as_mut().unwrap())
        .read_line(&mut ticket)
        .expect("read ticket");
    let result = recv(&home, ticket.trim(), &dir.path().join("out"), false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let mut text = String::new();
    BufReader::new(child.stderr.as_mut().unwrap())
        .read_to_string(&mut text)
        .expect("read the sender's lines");
    let _ = child.wait();

    let mut ids = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("receiver ") {
            let id = rest.split(' ').next().unwrap_or_default().to_string();
            assert_eq!(id.len(), 10, "{line}");
            assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "{line}");
            ids.push(id);
        }
    }
    assert!(text.contains(" connected directly"), "{text}");
    assert!(
        text.contains("finished, the whole transfer (directly)"),
        "{text}"
    );
    assert_eq!(ids.len(), 2, "one start and one finish line: {text}");
    assert_eq!(
        ids[0], ids[1],
        "the two lines name different receivers: {text}"
    );
}

/// A send that offers one place serves the receiver holding it and refuses a second endpoint while
/// that receiver is still transferring. The refusal is one line, and it names the limit.
#[test]
fn a_sender_refuses_a_receiver_when_the_places_are_taken() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("big.bin");
    std::fs::write(&file, data(4 << 20, 9)).unwrap();
    let mut sender = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .current_dir(dir.path())
        .args(["send", "big.bin", "--keep", "1", "--expires", "1h"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start send");
    let mut ticket = String::new();
    BufReader::new(sender.stdout.as_mut().unwrap())
        .read_line(&mut ticket)
        .expect("read ticket");

    // The first receiver is paced, so it is still connected when the second asks. Its own home
    // gives it a key of its own: a receiver binds a fresh endpoint for every run.
    let mut first = Command::new(EXE)
        .env("SLUICE_HOME", dir.path().join("home-one"))
        .current_dir(dir.path())
        .args(["recv", ticket.trim(), "--out", "one", "--limit-rate", "64K"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the first receiver");

    let (lines, reader) = sender_lines(&mut sender);
    wait_for(&lines, " connected ");
    let mut second = Command::new(EXE)
        .env("SLUICE_HOME", dir.path().join("home-two"))
        .current_dir(dir.path())
        .args(["recv", ticket.trim(), "--out", "two"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start the second receiver");
    let text = wait_for(&lines, "refused, the 1 places are taken");

    let _ = first.kill();
    let _ = second.kill();
    let _ = sender.kill();
    let _ = first.wait();
    let _ = second.wait();
    let _ = sender.wait();
    drop(reader);
    assert!(text.contains("refused, the 1 places are taken"), "{text}");
}

/// The sender's stderr as lines on a channel, so a test waits for a line with a deadline instead of
/// blocking for ever on a sender that never says it.
fn sender_lines(
    sender: &mut std::process::Child,
) -> (mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
    let stderr = sender.stderr.take().expect("the sender's stderr is piped");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    (rx, reader)
}

/// Waits for the first line that contains `wanted`, and returns every line read so far.
fn wait_for(lines: &mpsc::Receiver<String>, wanted: &str) -> String {
    let mut text = String::new();
    loop {
        let line = lines
            .recv_timeout(Duration::from_secs(30))
            .unwrap_or_else(|_| panic!("the sender never said {wanted:?}; it said: {text}"));
        text.push_str(&line);
        text.push('\n');
        if line.contains(wanted) {
            return text;
        }
    }
}
