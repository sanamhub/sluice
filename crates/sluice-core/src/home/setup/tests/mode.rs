//! A change of mode that did nothing, on Unix. A file system such as vfat or exFAT mounted with
//! `quiet` answers `chmod` with success and keeps the old mode.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::super::make_0700;
use crate::Error;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn changes_nothing(_: &Path) -> std::io::Result<()> {
    Ok(())
}

fn is_refused(_: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from_raw_os_error(libc::EPERM))
}

/// The mode is read back after `chmod`, and a home that is not 0700 is refused in the same words
/// as a refused `chmod`.
#[test]
fn a_change_of_mode_that_did_nothing_is_refused_like_one_that_failed() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let mut messages = Vec::new();
    for change in [changes_nothing, is_refused] {
        let Err(Error::Storage(message)) = make_0700(&path, change) else {
            panic!("a home that is not private was not refused");
        };
        messages.push(message);
    }
    for message in &messages {
        assert!(message.contains("cannot restrict access to"), "{message}");
        assert!(message.contains(&path.display().to_string()), "{message}");
        assert!(message.contains("keeps Unix permissions"), "{message}");
    }
    assert!(
        messages[0].contains("still has mode 0755"),
        "{}",
        messages[0]
    );
    assert_eq!(mode(&path), 0o755, "nothing was changed");
    // A change that works is the same call, and passes.
    assert!(
        make_0700(&path, |p| fs::set_permissions(
            p,
            fs::Permissions::from_mode(0o700)
        ))
        .is_ok()
    );
    assert_eq!(mode(&path), 0o700);
}
