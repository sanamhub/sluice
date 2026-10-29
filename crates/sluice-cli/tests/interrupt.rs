//! A receive that is interrupted: the private temporary store it made is removed, and the command
//! ends as cancelled rather than as a failure.
//!
//! The interrupt is sent as `SIGINT`, which a test can deliver on Unix only; on Windows a process
//! cannot raise a console control event in another process without a console of its own.
#![cfg(unix)]

mod common;

use common::{EXE, start_in};
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// The entries of `dir` whose names start with `prefix`.
fn entries(dir: &std::path::Path, prefix: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
        })
        .collect()
}

/// Waits until `check` returns a non-empty result, or fails the test naming what was awaited.
fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(found) = check() {
            return found;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// A receive killed with Ctrl-C leaves its temporary store behind only if nobody removes it, so the
/// interrupt handler drops the future and the folder goes with it. The partial store of a file
/// receive is beside the output and stays, for a later run to continue.
#[cfg(unix)]
#[test]
fn an_interrupted_receive_removes_the_temporary_store_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("big.bin");
    std::fs::write(&file, common::data(32 << 20, 9)).unwrap();
    let (sender, ticket) = start_in(
        dir.path(),
        &dir.path().join("home"),
        &["big.bin", "--expires", "1h", "--limit-rate", "4M"],
    );
    let recv_home = dir.path().join("recv-home");
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &recv_home)
        .args(["recv", &ticket, "--out", "-", "--limit-rate", "1M"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start recv");

    // The temporary store is made once the transfer starts fetching, and it is big enough that the
    // cap keeps the receive running long enough to interrupt it.
    let tmp = recv_home.join("tmp");
    wait_for("a temporary store", || {
        let found = entries(&tmp, ".sluice-tmp-");
        (!found.is_empty()).then_some(found)
    });
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send the interrupt");
    assert!(status.success(), "{status:?}");
    let status = child.wait().expect("wait for recv");
    assert_eq!(status.code(), Some(130), "the exit code of an interrupt");

    // The interrupt drops the receive, so the folder it made is removed before the process ends.
    wait_for("the temporary store to be removed", || {
        entries(&tmp, ".sluice-tmp-").is_empty().then_some(())
    });
    drop(sender);
}
