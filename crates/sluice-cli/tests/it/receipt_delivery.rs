//! `recv --receipt` and `send --no-receipt`: two processes, one transfer, and the file each leaves.

use std::fs;

use crate::common::{bare, data, recv_in, start, start_in, stdout_text, wait_for_sender};

/// A file, a sender for it, and a folder to receive into.
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("report.bin");
    fs::write(&source, data(4096, 3)).unwrap();
    let out = dir.path().join("out");
    (dir, home, source, out)
}

#[test]
fn a_receive_writes_the_receipt_and_the_file_verifies() {
    let (dir, home, source, out) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let receipt = dir.path().join("receipt.txt");
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            out.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "recv failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_for_sender(&mut sender);
    let received = out.join("report.bin");
    assert_eq!(fs::read(&received).unwrap(), fs::read(&source).unwrap());

    let text = fs::read_to_string(&receipt).unwrap();
    assert!(text.starts_with("sluicereceipt1"), "{text}");
    let assert = bare()
        .args(["receipt", "verify"])
        .arg(&receipt)
        .args(["--file"])
        .arg(&received)
        .assert()
        .success();
    assert!(
        stdout_text(&assert).contains("matches: its length and commitment"),
        "{}",
        stdout_text(&assert)
    );
}

#[test]
fn a_receive_without_the_flag_writes_no_receipt() {
    let (dir, home, source, out) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap()],
    );
    assert!(output.status.success(), "{output:?}");
    wait_for_sender(&mut sender);
    assert!(out.join("report.bin").exists());
    let entries: Vec<String> = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !entries.iter().any(|name| name.ends_with(".txt")),
        "a receipt was written: {entries:?}"
    );
}

#[test]
fn a_sender_that_cleared_the_bit_writes_no_receipt() {
    let (dir, home, source, out) = fixture();
    let (mut sender, ticket) = start_in(
        dir.path(),
        &home,
        &["report.bin", "--expires", "1h", "--no-receipt"],
    );
    let receipt = dir.path().join("receipt.txt");
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            out.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "recv failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_for_sender(&mut sender);
    assert!(out.join("report.bin").exists());
    assert!(!receipt.exists(), "a receipt was written");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no receipt arrived"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = &source;
}

/// A file at the receipt's name is not replaced without `--force`, and the receive says so before
/// it fetches anything.
#[test]
fn an_existing_receipt_file_is_not_replaced_without_force() {
    let (dir, home, source, out) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let receipt = dir.path().join("receipt.txt");
    fs::write(&receipt, "already here").unwrap();
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            out.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(6), "{output:?}");
    let stderr = stderr_text_of(&output);
    assert!(stderr.contains("already there"), "{stderr}");
    assert_eq!(fs::read_to_string(&receipt).unwrap(), "already here");
    sender.0.kill().ok();
}

/// With `--force` the receipt replaces the file, and the transferred content still matches it.
#[test]
fn force_replaces_the_receipt_file() {
    let (dir, home, source, out) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let receipt = dir.path().join("receipt.txt");
    fs::write(&receipt, "already here").unwrap();
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            out.to_str().unwrap(),
            "--receipt",
            receipt.to_str().unwrap(),
            "--force",
        ],
    );
    assert!(
        output.status.success(),
        "recv failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    wait_for_sender(&mut sender);
    assert!(
        fs::read_to_string(&receipt)
            .unwrap()
            .starts_with("sluicereceipt1")
    );
}

fn stderr_text_of(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
