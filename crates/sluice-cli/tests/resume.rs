//! A receive that was killed partway: the store it left, and the run that continues from it.

mod common;

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use common::{EXE, data, recv, start, start_in, wait_for, wait_for_sender};

#[test]
fn a_killed_receive_resumes_from_its_partial_store() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let file = dir.path().join("large.bin");
    let body = data(64 * 1024 * 1024, 11);
    std::fs::write(&file, &body).unwrap();

    let (mut sender, ticket) = start(&home, &file, "1h");
    std::fs::create_dir(&out).unwrap();
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", out.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start recv");
    wait_for_partial(&out);
    std::thread::sleep(Duration::from_millis(100));
    let _ = child.kill();
    let _ = child.wait();

    let result = recv(&home, &ticket, &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(out.join("large.bin")).unwrap(), body);
    // The partial store did its job and is gone after a successful write.
    assert!(!std::fs::read_dir(&out).unwrap().flatten().any(|entry| {
        entry
            .file_name()
            .to_string_lossy()
            .starts_with(".sluice-partial-")
    }));
    wait_for_sender(&mut sender);
}

/// Both files name the same content, so the store holds one blob for two targets. Writing the
/// first must not leave the second with nothing to read, here and after a run is killed.
#[test]
fn two_identical_files_arrive_and_a_killed_run_resumes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let out = dir.path().join("out");
    let source = dir.path().join("dup");
    std::fs::create_dir_all(source.join("small")).unwrap();
    let big = data(16 * 1024 * 1024, 15);
    std::fs::write(source.join("a.bin"), &big).unwrap();
    std::fs::write(source.join("b.bin"), &big).unwrap();
    let small = data(5000, 16);
    std::fs::write(source.join("small").join("c.txt"), &small).unwrap();
    std::fs::write(source.join("small").join("d.txt"), &small).unwrap();

    let (sender, ticket) = start_in(dir.path(), &home, &["dup", "--expires", "1h"]);
    std::fs::create_dir_all(&out).unwrap();
    let mut child = Command::new(EXE)
        .env("SLUICE_HOME", &home)
        .args(["recv", &ticket, "--out", out.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start recv");
    wait_for(&out.join("dup").join("a.bin"));
    let _ = child.kill();
    let _ = child.wait();
    drop(sender);

    // A restarted sender of the same folder prints the same root, so the store is still this
    // ticket's and the second run resumes instead of starting over.
    let (mut sender, ticket) = start_in(dir.path(), &home, &["dup", "--expires", "1h"]);
    let result = recv(&home, &ticket, &out, false);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(std::fs::read(out.join("dup/a.bin")).unwrap(), big);
    assert_eq!(std::fs::read(out.join("dup/b.bin")).unwrap(), big);
    assert_eq!(std::fs::read(out.join("dup/small/c.txt")).unwrap(), small);
    assert_eq!(std::fs::read(out.join("dup/small/d.txt")).unwrap(), small);
    wait_for_sender(&mut sender);
}

fn wait_for_partial(out: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let found = std::fs::read_dir(out).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".sluice-partial-")
        });
        if found {
            return;
        }
        assert!(Instant::now() < deadline, "no partial store appeared");
        std::thread::sleep(Duration::from_millis(20));
    }
}
