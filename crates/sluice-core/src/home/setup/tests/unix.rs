//! A home folder the user made before sluice ran, on Unix. The Windows half is in
//! `private_windows/tests.rs`, beside the access list it reads.
//!
//! A file system that keeps no Unix permissions, such as vfat, refuses the change of mode the way
//! a folder of another user does, with the same message. No test builds that case: mounting one
//! needs root.

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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

/// Whether this process has any effective capability, from `CapEff` in `/proc/self/status`. A
/// process with `CAP_FOWNER` may change the mode of a folder it does not own, as root may, so the
/// case below cannot be built there either. `None` where the file cannot be read, as off Linux.
fn has_capabilities() -> Option<bool> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let hex = status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:"))?;
    Some(u64::from_str_radix(hex.trim(), 16).ok()? != 0)
}

/// An empty folder that another user owns, which this one may list and not change. Only root can
/// make one, so the test looks for one the system already has. It gives none where it runs as
/// root, or with any effective capability, since either may change any folder.
fn empty_folder_of_another_user() -> Option<&'static Path> {
    let me = fs::metadata(tempfile::tempdir().ok()?.path()).ok()?.uid();
    if me == 0 || has_capabilities() == Some(true) {
        return None;
    }
    ["/srv", "/opt", "/media", "/mnt", "/home"]
        .into_iter()
        .map(Path::new)
        .find(|path| {
            fs::metadata(path).is_ok_and(|meta| meta.is_dir() && meta.uid() != me)
                && fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
        })
}

#[test]
fn a_fresh_home_whose_access_cannot_be_changed_is_a_storage_error() {
    let Some(path) = empty_folder_of_another_user() else {
        eprintln!(
            "SKIPPED: no empty folder of another user, or running as root or with a capability"
        );
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
