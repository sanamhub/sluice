//! `recv --delta-from`: two processes, one transfer, and what the summary says about where the
//! bytes came from.

mod common;

use std::fs;

use common::{bare, recv_in, start, wait_for_sender};

/// `len` bytes from a fixed seed that do not repeat, so the chunker finds real boundaries.
fn content(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 3) as u8
        })
        .collect()
}

const SIZE: usize = 3 << 20;

/// A new file, an older version of it that differs in one place, and a sender for the new one.
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    Vec<u8>,
) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let new = content(SIZE, 5);
    let mut old = new.clone();
    for byte in &mut old[1 << 20..(1 << 20) + 50_000] {
        *byte ^= 0x77;
    }
    let source = dir.path().join("report.bin");
    fs::write(&source, &new).unwrap();
    let older = dir.path().join("older.bin");
    fs::write(&older, &old).unwrap();
    (dir, home, source, older, new)
}

#[test]
fn the_summary_says_how_many_bytes_came_from_the_local_file() {
    let (dir, home, source, older, new) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let out = dir.path().join("out");
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &[
            "--out",
            out.to_str().unwrap(),
            "--delta-from",
            older.to_str().unwrap(),
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "recv failed: {stderr}");
    wait_for_sender(&mut sender);
    assert_eq!(fs::read(out.join("report.bin")).unwrap(), new);
    assert!(stderr.contains("from the local file"), "{stderr}");
    assert!(stderr.contains("from the network"), "{stderr}");
}

#[test]
fn json_carries_the_two_sums_and_they_add_up() {
    let (dir, home, source, older, _new) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let out = dir.path().join("out");
    let assert = bare()
        .env("SLUICE_HOME", &home)
        .args(["--json", "recv", &ticket, "--out"])
        .arg(&out)
        .arg("--delta-from")
        .arg(&older)
        .assert()
        .success();
    wait_for_sender(&mut sender);
    let text = common::stdout_text(&assert);
    let delta = text
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|value| value["event"] == "delta")
        .unwrap_or_else(|| panic!("no delta event in {text}"));
    let local = delta["local_bytes"].as_u64().unwrap();
    let network = delta["network_bytes"].as_u64().unwrap();
    assert_eq!(local + network, SIZE as u64);
    assert!(
        local > network,
        "{local} from the local file, {network} from the network"
    );
}

#[test]
fn without_the_flag_the_output_is_as_it_was() {
    let (dir, home, source, _older, new) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let out = dir.path().join("out");
    let output = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", out.to_str().unwrap()],
    );
    assert!(output.status.success());
    wait_for_sender(&mut sender);
    assert_eq!(fs::read(out.join("report.bin")).unwrap(), new);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("local file"), "{stderr}");
}

#[test]
fn a_path_that_is_not_there_or_a_mode_with_no_file_is_refused() {
    let (dir, home, source, older, _new) = fixture();
    let (mut sender, ticket) = start(&home, &source, "1h");
    let out = dir.path().join("out");
    let missing = dir.path().join("absent.bin");
    for (args, said) in [
        (
            vec![
                "--out",
                out.to_str().unwrap(),
                "--delta-from",
                missing.to_str().unwrap(),
            ],
            "cannot be read",
        ),
        (
            vec!["--out", "-", "--delta-from", older.to_str().unwrap()],
            "standard output",
        ),
        (
            vec![
                "--out",
                out.to_str().unwrap(),
                "--range",
                "0:10",
                "--delta-from",
                older.to_str().unwrap(),
            ],
            "byte range",
        ),
    ] {
        let output = recv_in(dir.path(), &home, &ticket, &args);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(said), "{stderr}");
    }
    sender.0.kill().ok();
}
