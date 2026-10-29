//! A sender names each receiver it serves: a short fingerprint, how the connection reached it, and
//! whether the receiver took the whole transfer.

mod common;

use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
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
