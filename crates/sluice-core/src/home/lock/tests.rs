//! The lock and the pid file are plain files of the home. A FIFO at either must answer at once and
//! be refused, and the pid file is read in bounded bytes.

use std::fs;
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::time::{Duration, Instant};

use super::read_pid;
#[cfg(unix)]
use super::take;
#[cfg(unix)]
use crate::Error;

#[cfg(unix)]
fn make_fifo(path: &Path) -> bool {
    std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

fn home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("home")).unwrap();
    dir
}

#[test]
fn the_pid_file_is_read_as_a_number_and_at_most_32_bytes() {
    let dir = home();
    let home = dir.path().join("home");
    assert_eq!(read_pid(&home).unwrap(), None);
    fs::write(home.join("lock.pid"), "4242\n").unwrap();
    assert_eq!(read_pid(&home).unwrap(), Some(4242));
    // A file with no end to its first line is read to 32 bytes and nothing is parsed.
    fs::write(home.join("lock.pid"), "1".repeat(64)).unwrap();
    assert_eq!(read_pid(&home).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_lock_is_refused_at_once() {
    use std::os::unix::fs::FileTypeExt;
    let dir = home();
    let home = dir.path().join("home");
    let lock = home.join("lock");
    if !make_fifo(&lock) {
        eprintln!("SKIPPED: mkfifo is not available");
        return;
    }
    let start = Instant::now();
    let Err(Error::Policy(message)) = take(&home) else {
        panic!("a FIFO as the lock was accepted");
    };
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "the open waited for a writer"
    );
    assert!(message.contains("is not a file"), "{message}");
    assert!(message.contains(&lock.display().to_string()), "{message}");
    assert!(
        fs::symlink_metadata(&lock).unwrap().file_type().is_fifo(),
        "the FIFO is left as it is"
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_at_the_pid_file_is_refused_at_once() {
    use std::os::unix::fs::FileTypeExt;
    let dir = home();
    let home = dir.path().join("home");
    let pid = home.join("lock.pid");
    fs::write(home.join("lock"), "").unwrap();
    if !make_fifo(&pid) {
        eprintln!("SKIPPED: mkfifo is not available");
        return;
    }
    let start = Instant::now();
    let Err(Error::Policy(message)) = take(&home) else {
        panic!("a FIFO as the pid file was accepted");
    };
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "the read waited for a writer"
    );
    assert!(message.contains("is not a file"), "{message}");
    assert!(message.contains(&pid.display().to_string()), "{message}");
    assert!(
        fs::symlink_metadata(&pid).unwrap().file_type().is_fifo(),
        "the FIFO is left as it is"
    );
}
