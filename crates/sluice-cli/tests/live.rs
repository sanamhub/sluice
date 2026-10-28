//! A live file through two real processes: the sender follows it while the receiver writes it.

mod common;

use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

use common::{EXE, recv_in, start_in, wait_for_sender};

#[test]
fn a_live_file_grows_while_the_receiver_watches() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("grow.bin");
    std::fs::write(&source, vec![1u8; 600 * 1024]).unwrap();
    let (mut sender, ticket) = start_in(
        dir.path(),
        &home,
        &[
            "grow.bin",
            "--live",
            "--close-after",
            "1",
            "--expires",
            "1h",
        ],
    );
    let out = dir.path().join("out.bin");
    let child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", out.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");
    std::thread::sleep(Duration::from_millis(300));
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap();
        file.write_all(&vec![2u8; 900 * 1024]).unwrap();
        file.sync_all().unwrap();
    }
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(&source).unwrap()
    );
    wait_for_sender(&mut sender);
}

#[test]
fn a_live_ticket_with_a_default_out_is_refused_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("named.bin");
    std::fs::write(&source, b"some bytes").unwrap();
    let (_sender, ticket) = start_in(
        dir.path(),
        &home,
        &[
            "named.bin",
            "--live",
            "--close-after",
            "1",
            "--expires",
            "1h",
        ],
    );
    let result = recv_in(dir.path(), &home, &ticket, &[]);
    assert_eq!(result.status.code(), Some(6));
    let text = String::from_utf8_lossy(&result.stderr);
    assert!(text.contains("--out must name the file"), "{text}");
}

#[test]
fn a_range_on_a_live_ticket_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("ranged.bin");
    std::fs::write(&source, b"some bytes").unwrap();
    let (_sender, ticket) = start_in(
        dir.path(),
        &home,
        &[
            "ranged.bin",
            "--live",
            "--close-after",
            "1",
            "--expires",
            "1h",
        ],
    );
    let result = recv_in(
        dir.path(),
        &home,
        &ticket,
        &["--out", "out.bin", "--range", "0:1"],
    );
    assert_eq!(result.status.code(), Some(6));
    assert!(String::from_utf8_lossy(&result.stderr).contains("not on a live one"));
}

/// `-o f` names a file in the folder the command runs in. The free-space floor is read from that
/// folder, so the empty path a bare name's parent looks like must not reach the volume call.
#[test]
fn a_live_receive_writes_a_file_named_in_the_working_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let source = dir.path().join("grow.bin");
    std::fs::write(&source, vec![1u8; 600 * 1024]).unwrap();
    let (mut sender, ticket) = start_in(
        dir.path(),
        &home,
        &[
            "grow.bin",
            "--live",
            "--close-after",
            "1",
            "--expires",
            "1h",
        ],
    );
    let child = Command::new(EXE)
        .current_dir(dir.path())
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", "f"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        std::fs::read(dir.path().join("f")).unwrap(),
        std::fs::read(&source).unwrap()
    );
    wait_for_sender(&mut sender);
}
