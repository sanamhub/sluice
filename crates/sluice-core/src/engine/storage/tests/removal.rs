//! Removing what a command made: one attempt, then a bounded wait for a file that is still held.

/// A file that a scanner or a store's own close holds for a moment is released while the removal
/// keeps trying, and the folder still goes without the command seeing a failure.
#[cfg(windows)]
#[test]
fn a_folder_held_for_a_moment_is_still_removed() {
    use std::{
        os::windows::fs::OpenOptionsExt,
        time::{Duration, Instant},
    };

    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("folder");
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("held");
    std::fs::write(&file, b"held").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    let released = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        drop(held);
    });
    let started = Instant::now();
    super::super::remove_quietly(&folder);
    released.join().unwrap();
    assert!(!folder.exists(), "the folder stayed");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the removal waited for {:?}",
        started.elapsed()
    );
}

/// Only an error a moment can clear is worth another try: a name that is gone or a path that cannot
/// be removed at all is not, so the command is not delayed for it.
#[cfg(windows)]
#[test]
fn only_a_held_name_is_tried_again() {
    for code in [5, 32, 33, 145] {
        assert!(
            super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
    for code in [2, 3, 267] {
        assert!(
            !super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
}

#[cfg(unix)]
#[test]
fn only_a_busy_name_is_tried_again() {
    for code in [16, 39] {
        assert!(
            super::super::retryable(&std::io::Error::from_raw_os_error(code)),
            "{code}"
        );
    }
    assert!(!super::super::retryable(
        &std::io::Error::from_raw_os_error(2)
    ));
}
