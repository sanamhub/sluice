//! A home folder the user made before sluice ran, on Unix. The Windows half is in
//! `private_windows/tests.rs`, beside the access list it reads.
//!
//! A file system that keeps no Unix permissions, such as vfat, refuses the change of mode the way
//! a folder of another user does, with the same message. No test mounts one, which needs root:
//! `mode.rs` stands in for it, and the folder of another user comes from the test job of `ci.yml`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

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

/// An empty folder that another user owns, which only root can make. The test job of `ci.yml` makes
/// one with `sudo` and puts its path in `SLUICE_ROOT_OWNED_DIR`; outside CI the test looks for one
/// the system already has. A CI run without the variable fails: this test is the one that proves
/// another user's folder is refused, and it must not pass by not looking. The folder is made
/// readable, so the open reaches the change of mode and is refused there.
fn root_owned_empty_folder() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SLUICE_ROOT_OWNED_DIR") {
        return Some(PathBuf::from(path));
    }
    assert!(
        std::env::var_os("CI").is_none(),
        "CI is set, and SLUICE_ROOT_OWNED_DIR is not: the test job makes a root-owned folder"
    );
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
        .map(Path::to_path_buf)
}

/// A home another user owns is refused as theirs, and not as a storage error about permissions:
/// the owner can change whatever sluice sets, so the way out is to take the folder or use another
/// one. The folder is left exactly as it was.
#[test]
fn a_home_another_user_owns_is_refused_as_theirs() {
    let Some(path) = root_owned_empty_folder() else {
        eprintln!("SKIPPED: no empty folder of another user was found to test the refusal with");
        return;
    };
    let before = mode(&path);
    let Err(Error::Policy(message)) = Home::open(&path) else {
        panic!("a home another user owns was accepted");
    };
    assert!(message.contains("belongs to another user"), "{message}");
    assert_eq!(mode(&path), before, "the folder's mode is as it was");
    assert!(
        fs::read_dir(&path).unwrap().next().is_none(),
        "nothing was written"
    );
}
