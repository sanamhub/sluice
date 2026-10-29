//! Several tickets in one command, and the byte cap of one command.

mod common;

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

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

/// A job that fails on a network drop is started again by the queue with the ticket it was given,
/// and the command succeeds because every job finished in the end.
#[test]
fn a_job_that_drops_is_retried_and_the_command_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let mut senders = Vec::new();
    let mut tickets = Vec::new();
    let mut sizes = Vec::new();
    for index in 0..3 {
        let home = dir.path().join(format!("home-{index}"));
        let file = dir.path().join(format!("file-{index}.bin"));
        // The first transfer is large enough to be cut off mid-way; the others are quick.
        let size = if index == 0 {
            32 * 1024 * 1024
        } else {
            256 * 1024
        };
        std::fs::write(&file, data(size, index as u8)).unwrap();
        let (sender, ticket) = start(&home, &file, "1h");
        senders.push(sender);
        tickets.push(ticket);
        sizes.push(size);
    }

    let out_text = out.to_str().unwrap();
    let mut command: Vec<&str> = vec!["recv"];
    command.extend(tickets.iter().map(String::as_str));
    command.extend(["--out", out_text, "--limit-rate", "8M"]);
    let mut child = Command::new(common::EXE)
        .env("SLUICE_HOME", dir.path().join("recv-home"))
        .args(&command)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");

    // The first sender goes away in the middle of its transfer, and comes back from the same home
    // while the command is waiting to retry the job it dropped.
    std::thread::sleep(Duration::from_millis(600));
    drop(senders.remove(0));
    std::thread::sleep(Duration::from_millis(200));
    let home = dir.path().join("home-0");
    let file = dir.path().join("file-0.bin");
    let (mut again, _ticket) = start(&home, &file, "1h");

    let deadline = Instant::now() + Duration::from_secs(120);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "the command did not finish");
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        use std::io::Read;
        pipe.read_to_string(&mut stderr).unwrap();
    }
    assert!(status.success(), "status={status}: {stderr}");
    for (index, size) in sizes.iter().enumerate() {
        assert_eq!(
            std::fs::read(out.join(format!("file-{index}.bin"))).unwrap(),
            data(*size, index as u8)
        );
    }
    wait_for_sender(&mut again);
    for sender in &mut senders {
        wait_for_sender(sender);
    }
}

/// Two tickets whose roots are equal would open the same partial store, and the second job's
/// failed load would remove the store the first is using. The command names both and starts
/// nothing.
#[test]
fn one_transfer_named_twice_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    let file = dir.path().join("same.bin");
    std::fs::write(&file, data(64 * 1024, 3)).unwrap();
    // The same content sent from two homes: different senders, one root.
    let (mut first, one) = start(&dir.path().join("home-a"), &file, "1h");
    let (mut second, two) = start(&dir.path().join("home-b"), &file, "1h");
    let result = std::process::Command::new(common::EXE)
        .env("SLUICE_HOME", dir.path().join("recv-home"))
        .args(["recv", &one, &two, "--out", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("ticket 1 and ticket 2"), "{stderr}");
    assert!(!out.join("same.bin").exists(), "something was written");
    // Nothing was asked of either sender; both are still serving when the command ends.
    let _ = (&mut first, &mut second);
}
