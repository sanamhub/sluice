//! Several tickets in one command, and the byte cap of one command.

mod common;

use std::time::{Duration, Instant};

use common::{data, recv, start, start_in, wait_for_sender};

#[test]
fn three_tickets_run_together_and_write_every_file() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let mut senders = Vec::new();
    let mut tickets = Vec::new();
    for index in 0..3 {
        let home = dir.path().join(format!("home-{index}"));
        let file = dir.path().join(format!("file-{index}.bin"));
        std::fs::write(&file, data(256 * 1024, index as u8)).unwrap();
        let (sender, ticket) = start(&home, &file, "1h");
        senders.push(sender);
        tickets.push(ticket);
    }
    let out_text = out.to_str().unwrap();
    let mut command: Vec<&str> = vec!["recv"];
    command.extend(tickets.iter().map(String::as_str));
    command.extend(["--out", out_text, "--priority", "high"]);
    let result = std::process::Command::new(common::EXE)
        .env("SLUICE_HOME", dir.path().join("recv-home"))
        .args(&command)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for index in 0..3 {
        assert_eq!(
            std::fs::read(out.join(format!("file-{index}.bin"))).unwrap(),
            data(256 * 1024, index as u8)
        );
    }
    for sender in &mut senders {
        wait_for_sender(sender);
    }
}

/// A cap slows a transfer down by about its size divided by the rate. The test allows a quarter
/// either side, because a loopback transfer also spends time on setup and on the last bytes.
#[test]
fn a_rate_cap_slows_a_receive() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("capped.bin");
    let body = data(2 * 1024 * 1024, 4);
    std::fs::write(&file, &body).unwrap();
    let (mut sender, ticket) = start_in(
        dir.path(),
        &home,
        &["capped.bin", "--expires", "1h", "--limit-rate", "1M"],
    );
    let started = Instant::now();
    let result = recv(&home, &ticket, &out, false);
    let took = started.elapsed();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(out.join("capped.bin")).unwrap(), body);
    assert!(
        took >= Duration::from_millis(1500),
        "2 MiB at 1 MiB/s took only {took:?}"
    );
    assert!(
        took < Duration::from_secs(10),
        "the cap was far too slow: {took:?}"
    );
    wait_for_sender(&mut sender);
}
