//! A change of mode that did nothing, on Unix. A file system such as vfat or exFAT mounted with
//! `quiet` answers `chmod` with success and keeps the old mode.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use rustix::fs::Mode;

use super::super::private::make_0700_with;
use crate::Error;

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn changes_nothing(_: &File) -> io::Result<()> {
    Ok(())
}

fn is_refused(_: &File) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::EPERM))
}

fn sets_0700(file: &File) -> io::Result<()> {
    rustix::fs::fchmod(file, Mode::from_raw_mode(0o700)).map_err(io::Error::from)
}

/// The mode is read back through the same handle after `fchmod`, and a home that is not 0700 is
/// refused in the same words as a refused `fchmod`.
#[test]
fn a_change_of_mode_that_did_nothing_is_refused_like_one_that_failed() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let me = rustix::process::geteuid().as_raw();
    let mut messages = Vec::new();
    for change in [changes_nothing, is_refused] {
        let Err(Error::Storage(message)) = make_0700_with(&path, me, change) else {
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
    assert!(make_0700_with(&path, me, sets_0700).is_ok());
    assert_eq!(mode(&path), 0o700);
}

/// A folder this user may not open is refused by its owner, which `lstat` reads without opening it.
/// When the owner is not the user the check is for, that is another user's folder; when it is, the
/// file system is what refused the change, and the storage error stays.
#[test]
fn a_folder_that_cannot_be_opened_is_refused_by_its_owner() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let me = rustix::process::geteuid().as_raw();
    let Err(Error::Policy(message)) = make_0700_with(&path, me.wrapping_add(1), sets_0700) else {
        panic!("a folder that cannot be opened was not refused");
    };
    assert!(message.contains("belongs to another user"), "{message}");
    let Err(Error::Storage(message)) = make_0700_with(&path, me, sets_0700) else {
        panic!("a folder that cannot be opened was not refused");
    };
    assert!(message.contains("cannot restrict access to"), "{message}");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
}
