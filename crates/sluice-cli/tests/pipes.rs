//! Pipes: a ticket on standard input, a raw blob from standard input, and content written to
//! standard output instead of a file.

mod common;

use std::{
    io::Write,
    process::{Command, Stdio},
};

use common::{EXE, data, start, start_stdin, wait_for_sender};

/// The store for a write to standard output lives under the home's private `tmp`, and it is gone
/// when the command ends.
fn assert_no_temporary_store(home: &std::path::Path) {
    let left = std::fs::read_dir(home.join("tmp"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "a temporary store was left under the home");
}

#[test]
fn a_ticket_on_standard_input_is_read() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let landing = dir.path().join("landing");
    std::fs::create_dir(&landing).unwrap();
    let file = dir.path().join("piped.bin");
    let body = data(4096, 9);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .current_dir(&landing)
        .args(["recv", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(format!("{ticket}\n").as_bytes())
        .unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(landing.join("piped.bin")).unwrap(), body);
    wait_for_sender(&mut sender);
}

#[test]
fn standard_input_crosses_as_a_raw_blob() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let body = data(200 * 1024, 5);
    let (mut sender, ticket) = start_stdin(&home, &body, "1h");

    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, body);
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("wrote 204800 bytes"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_no_temporary_store(&home);
    wait_for_sender(&mut sender);
}

#[test]
fn an_empty_pipe_is_a_valid_raw_blob() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let (mut sender, ticket) = start_stdin(&home, b"", "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stdout.is_empty());
    assert_no_temporary_store(&home);
    wait_for_sender(&mut sender);
}

#[test]
fn a_one_file_transfer_goes_to_standard_output() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("one.bin");
    let body = data(150 * 1024, 8);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(result.stdout, body);
    assert_no_temporary_store(&home);
    wait_for_sender(&mut sender);
}

#[test]
fn a_folder_is_refused_on_standard_output() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("a.txt"), b"one").unwrap();
    std::fs::write(tree.join("b.txt"), b"two").unwrap();

    let (sender, ticket) = start(&home, &tree, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert_eq!(result.status.code(), Some(6));
    assert!(result.stdout.is_empty());
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(
        text.contains("folder cannot be written to standard output"),
        "{text}"
    );
    drop(sender);
}

#[test]
fn json_cannot_share_standard_output_with_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let file = dir.path().join("json.bin");
    std::fs::write(&file, b"bytes").unwrap();
    let (_sender, ticket) = start(&home, &file, "1h");
    let result = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["--json", "recv", &ticket, "--out", "-"])
        .output()
        .expect("run recv");
    assert_eq!(result.status.code(), Some(6));
    assert!(result.stdout.is_empty());
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("--out - cannot share it"), "{text}");
}
