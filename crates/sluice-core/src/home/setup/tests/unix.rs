//! A home folder the user made before sluice ran, on Unix. The Windows half is in
//! `private_windows/tests.rs`, beside the access list it reads.
//!
//! A file system that keeps no Unix permissions, such as vfat, refuses the change of mode the way
//! a folder of another user does, with the same message. No test mounts one, which needs root:
//! `mode.rs` stands in for it, and the folder of another user is looked for below.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::doctor::{Status, examine_at};
use crate::{Error, Home};

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn failed(home: &Path) -> Vec<String> {
    examine_at(home, true)
        .iter()
        .filter(|check| check.status() == Status::Failed)
        .map(|check| format!("{:?}", check.finding()))
        .collect()
}

#[test]
fn an_empty_folder_made_by_hand_becomes_private_and_doctor_passes_it() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    drop(Home::open(&path).unwrap().identity().unwrap());
    assert_eq!(mode(&path), 0o700);
    assert_eq!(failed(&path), Vec::<String>::new());
}

#[test]
fn a_folder_that_holds_an_unrelated_file_is_not_touched() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("notes.txt"), "mine").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    drop(Home::open(&path).unwrap().identity().unwrap());
    assert_eq!(
        mode(&path),
        0o755,
        "a folder that holds anything keeps its mode"
    );
    assert_eq!(fs::read_to_string(path.join("notes.txt")).unwrap(), "mine");
    let failed = failed(&path);
    assert_eq!(failed, ["AccessOpenToOthers(493)"], "0o755 is 493");
}

#[test]
fn a_folder_that_holds_only_a_hidden_file_is_not_new_and_is_left_alone() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    // What macOS Finder leaves in a folder it has shown.
    fs::write(path.join(".DS_Store"), "").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    drop(Home::open(&path).unwrap());
    assert_eq!(
        mode(&path),
        0o755,
        "a folder that holds a hidden file keeps its mode"
    );
}

/// An empty folder that another user owns, which this one may list and not change. Only root can
/// make one, so the test looks for one the system already has, and asks the system whether it is
/// one: `chmod` to the mode it has already must be refused with `EPERM`, which is what a folder of
/// another user answers, and not what an owner, root, or a process with `CAP_FOWNER` gets. It
/// guesses nothing from who owns the temporary folder, and the open below is only made on a folder
/// that has just refused.
fn empty_folder_of_another_user() -> Option<&'static Path> {
    ["/srv", "/opt", "/media", "/mnt", "/home"]
        .into_iter()
        .map(Path::new)
        .find(|path| {
            let Ok(meta) = fs::metadata(path) else {
                return false;
            };
            let empty = fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none());
            let same = fs::Permissions::from_mode(meta.permissions().mode() & 0o7777);
            let refused = fs::set_permissions(path, same)
                .is_err_and(|e| e.raw_os_error() == Some(libc::EPERM));
            meta.is_dir() && empty && refused
        })
}

#[test]
fn a_fresh_home_whose_access_cannot_be_changed_is_a_storage_error() {
    let Some(path) = empty_folder_of_another_user() else {
        eprintln!("SKIPPED: no empty folder that refuses this user a change of mode");
        return;
    };
    let before = mode(path);
    let Err(Error::Storage(message)) = Home::open(path) else {
        panic!("expected a storage error for {}", path.display());
    };
    assert!(message.contains("restrict access to"), "{message}");
    assert!(message.contains("keeps Unix permissions"), "{message}");
    assert_eq!(mode(path), before);
    assert!(
        fs::read_dir(path).unwrap().next().is_none(),
        "nothing was written"
    );
}
