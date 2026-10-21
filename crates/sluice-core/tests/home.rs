//! Opening a home, its lock and its identity key, through the public API and real processes.

use std::fs;
use std::path::Path;

use sluice_core::{Error, Home};

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn open_creates_the_folders_and_the_version_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new").join("home");
    let home = Home::open(&path).unwrap();
    assert_eq!(home.path(), path);
    assert_eq!(names(&path), ["VERSION", "blobs", "db", "logs"]);
    assert_eq!(fs::read_to_string(path.join("VERSION")).unwrap(), "1\n");
    // The identity is made when asked for, not when the home is opened.
    assert!(!path.join("identity.key").exists());
}

#[cfg(unix)]
#[test]
fn on_unix_the_folders_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("home");
    let _home = Home::open(&path).unwrap();
    for folder in ["", "blobs", "db", "logs"] {
        let mode = fs::metadata(path.join(folder))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{folder}");
    }
}

#[test]
fn reopening_a_home_changes_nothing_in_it() {
    let dir = tempfile::tempdir().unwrap();
    drop(Home::open(dir.path()).unwrap());
    let before = names(dir.path());
    drop(Home::open(dir.path()).unwrap());
    assert_eq!(names(dir.path()), before);
    assert_eq!(
        fs::read_to_string(dir.path().join("VERSION")).unwrap(),
        "1\n"
    );
}

#[test]
fn a_newer_layout_is_refused_with_both_versions_and_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "2\n").unwrap();
    let Err(Error::Policy(message)) = Home::open(dir.path()) else {
        panic!("expected a policy error");
    };
    assert!(
        message.contains("version 2") && message.contains("version 1"),
        "{message}"
    );
    assert_eq!(names(dir.path()), ["VERSION"]);
    assert_eq!(
        fs::read_to_string(dir.path().join("VERSION")).unwrap(),
        "2\n"
    );
}

#[test]
fn a_version_file_that_holds_no_version_is_a_storage_error_and_is_left_alone() {
    for text in [
        "",
        "abc\n",
        "-1\n",
        "+1\n",
        "0\n",
        "1.0\n",
        "99999999999\n",
        " 1\n",
    ] {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("VERSION"), text).unwrap();
        assert!(
            matches!(Home::open(dir.path()), Err(Error::Storage(_))),
            "{text:?}"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("VERSION")).unwrap(),
            text
        );
    }
}

#[test]
fn a_version_file_with_a_windows_line_end_is_read() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "1\r\n").unwrap();
    assert!(Home::open(dir.path()).is_ok());
}

#[test]
fn a_path_that_cannot_be_a_folder_is_a_storage_error_that_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a-file");
    fs::write(&file, "x").unwrap();
    let Err(Error::Storage(message)) = Home::open(&file) else {
        panic!("expected a storage error");
    };
    assert!(message.contains("a-file"), "{message}");
}
