//! The Windows half: a hidden `desktop.ini`, which Explorer writes into a folder whose look the
//! user changed, makes a folder not empty; and the paths Win32 would read as another folder.

use std::fs;
use std::path::PathBuf;

use super::{failed, is_private, open_folder};
use crate::Home;

#[test]
fn a_folder_that_holds_only_a_hidden_desktop_ini_is_not_new_and_is_left_alone() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    open_folder(&path);
    let ini = path.join("desktop.ini");
    fs::write(&ini, "[.ShellClassInfo]\r\n").unwrap();
    // The hidden attribute, as Explorer sets it. `attrib` needs no right a user lacks.
    let status = std::process::Command::new("attrib")
        .arg("+h")
        .arg(&ini)
        .status()
        .unwrap();
    assert!(status.success());
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN;
        let attributes = fs::metadata(&ini).unwrap().file_attributes();
        assert_ne!(attributes & FILE_ATTRIBUTE_HIDDEN, 0, "the file is hidden");
    }
    drop(Home::open(&path).unwrap());
    assert!(
        !is_private(&path),
        "a folder that holds a hidden file keeps its access"
    );
}

/// A path through `\\?\`, which Windows takes literally: a component that ends in a dot is a
/// folder of that name. Through any other path Win32 drops the dot and names another folder.
fn verbatim(parent: &std::path::Path) -> PathBuf {
    fs::canonicalize(parent).unwrap()
}

/// A home whose resolved path has a trailing-dot component. Before the verbatim path was kept,
/// `\\?\` was stripped and every call read `trail` and not `trail.`, a folder that was not there.
#[test]
fn a_home_with_a_trailing_dot_component_is_that_folder_and_not_another() {
    let parent = tempfile::tempdir().unwrap();
    let base = verbatim(parent.path());
    let home = base.join("trail.");
    fs::create_dir(&home).unwrap();
    Home::open(&home).unwrap().identity().unwrap();
    for name in ["blobs", "db", "logs", "VERSION", "identity.key"] {
        assert!(home.join(name).exists(), "{name} in trail.");
    }
    assert!(!base.join("trail").exists(), "no folder without the dot");
    assert!(is_private(&home));
    assert_eq!(failed(&home), []);
}

/// A home over 260 characters. `std` adds `\\?\` to a path that long for its own calls, but
/// doctor's calls to `GetNamedSecurityInfoW` and `GetDiskFreeSpaceExW` took the path as it was, and
/// failed.
#[test]
fn a_home_longer_than_260_characters_opens_and_doctor_passes_it() {
    let parent = tempfile::tempdir().unwrap();
    let mut long = verbatim(parent.path());
    while long.as_os_str().len() < 300 {
        long.push("a_folder_with_a_name_of_some_length_to_add_up");
    }
    fs::create_dir_all(&long).unwrap();
    let given = PathBuf::from(long.to_str().unwrap().strip_prefix(r"\\?\").unwrap());
    assert!(given.as_os_str().len() > 260);
    Home::open(&given).unwrap().identity().unwrap();
    assert!(is_private(&long));
    assert_eq!(failed(&given), []);
}
