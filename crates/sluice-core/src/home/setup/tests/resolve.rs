//! Resolving the home path: a link is refused even when the file system cannot say where it
//! leads, and a home that is not a link is taken as it is.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::super::{cannot_resolve, resolve, resolve_with};
use super::link_folder;
use crate::{Error, Home};

#[test]
fn a_link_in_place_of_blobs_is_refused_and_named() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    let elsewhere = parent.path().join("elsewhere");
    fs::create_dir(&path).unwrap();
    fs::create_dir(&elsewhere).unwrap();
    link_folder(&elsewhere, &path.join("blobs"));
    let Err(Error::Policy(message)) = Home::open(&path) else {
        panic!("a link as blobs was not refused");
    };
    assert!(message.contains("blobs"), "{message}");
    assert!(message.contains("is a link"), "{message}");
}

/// A home that is itself a link, with a link as its `blobs`: the message names the home as it was
/// given, and not the folder the link leads to.
#[test]
fn a_linked_home_whose_blobs_is_a_link_is_named_by_the_path_given() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let given = parent.path().join("home");
    let elsewhere = parent.path().join("elsewhere");
    for folder in [&target, &elsewhere] {
        fs::create_dir(folder).unwrap();
    }
    link_folder(&target, &given);
    link_folder(&elsewhere, &target.join("blobs"));
    let Err(Error::Policy(message)) = Home::open(&given) else {
        panic!("a link as blobs was not refused");
    };
    assert!(
        message.contains(&given.join("blobs").display().to_string()),
        "{message}"
    );
    assert!(
        !message.contains(&target.display().to_string()),
        "the target is not named: {message}"
    );
}

#[test]
fn a_resolved_path_is_absolute_and_is_what_the_system_gave() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = resolve(dir.path()).unwrap();
    assert!(resolved.is_absolute(), "{}", resolved.display());
    // On Windows that is the verbatim form, which every call that touches the home takes.
    assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());
    #[cfg(windows)]
    assert!(resolved.display().to_string().starts_with(r"\\?\"));
}

/// The verb says what went wrong: a path that is not there is not found, and one that is refused
/// is not "not found".
#[test]
fn an_error_that_cannot_resolve_has_a_verb_that_fits_it() {
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Storage(message)) = resolve(&dir.path().join("missing")) else {
        panic!("a missing home was resolved");
    };
    assert!(message.starts_with("cannot find "), "{message}");
    let path = Path::new("/some/home");
    for (kind, verb) in [
        (io::ErrorKind::NotFound, "find"),
        (io::ErrorKind::PermissionDenied, "get access to"),
        (io::ErrorKind::InvalidData, "resolve"),
    ] {
        let Error::Storage(message) = cannot_resolve(path, &io::Error::from(kind)) else {
            panic!("not a storage error");
        };
        let start = format!("cannot {verb} {}", path.display());
        assert!(message.starts_with(&start), "{message}");
    }
}

fn unsupported(_: &Path) -> io::Result<PathBuf> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

fn denied(_: &Path) -> io::Result<PathBuf> {
    Err(io::Error::from(io::ErrorKind::PermissionDenied))
}

/// A RAM disk, a WinFsp or Dokan drive, a VirtualBox share or an old SMB server cannot be asked
/// where a path leads. A home on one that is not a link is the path given, and the test runs from a
/// canonicalized temporary folder so a system link above it, such as macOS's `/var`, is not in the
/// path: those are the system's, and the fallback has to accept them.
#[test]
fn a_file_system_that_cannot_be_asked_gives_the_path_when_the_home_is_not_a_link() {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    assert_eq!(resolve_with(&base, unsupported).unwrap(), base);
    // A link on such a file system is refused: nothing says where it leads.
    let target = base.join("target");
    let link = base.join("link");
    fs::create_dir(&target).unwrap();
    link_folder(&target, &link);
    assert!(resolve_with(&link, unsupported).is_err());
    // A missing home is not found, and a refusal is not "unsupported".
    assert!(resolve_with(&base.join("missing"), unsupported).is_err());
    let Err(Error::Storage(message)) = resolve_with(&base, denied) else {
        panic!("a refusal was resolved");
    };
    assert!(message.starts_with("cannot get access to "), "{message}");
}

/// A file system that cannot resolve a path cannot say where a link in the middle of it leads
/// either, so every ancestor is checked, and a link is refused by name. The base is canonicalized
/// for the same reason as the test above.
#[test]
fn a_link_in_the_path_is_refused_when_the_file_system_cannot_resolve() {
    let parent = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(parent.path()).unwrap();
    let target = base.join("target");
    let link = base.join("link");
    fs::create_dir(&target).unwrap();
    fs::create_dir(target.join("home")).unwrap();
    link_folder(&target, &link);
    let home = link.join("home");
    let Err(Error::Policy(message)) = resolve_with(&home, unsupported) else {
        panic!("a link in the path was resolved");
    };
    assert!(
        message.contains(&link.display().to_string()),
        "the link is named: {message}"
    );
    assert!(message.contains("is a link"), "{message}");
    // The same path with no link is taken as it is, and is absolute.
    let plain = target.join("home");
    let resolved = resolve_with(&plain, unsupported).unwrap();
    assert_eq!(resolved, plain);
    assert!(resolved.is_absolute());
}

#[cfg(windows)]
#[test]
fn error_invalid_function_is_the_same_answer_as_unsupported() {
    fn invalid_function(_: &Path) -> io::Result<PathBuf> {
        Err(io::Error::from_raw_os_error(1))
    }
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    assert_eq!(resolve_with(&base, invalid_function).unwrap(), base);
}
