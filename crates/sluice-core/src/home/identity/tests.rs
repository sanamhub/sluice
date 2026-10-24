//! A key another user owns, a key with a second name, a key that is a link and a key that is a
//! folder: what `Home::identity` and `Home::rotate_identity` refuse. The owner check is a
//! parameter, because a key of another user cannot be made without being root.

use std::fs::{self, File};
use std::io;

use super::{load_or_create_with, rotate_with};
use crate::{Error, Home};

fn ours(_: &File) -> io::Result<bool> {
    Ok(true)
}

fn not_ours(_: &File) -> io::Result<bool> {
    Ok(false)
}

fn open_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let store = Home::open(dir.path()).unwrap();
    store.identity().unwrap();
    dir
}

#[test]
fn a_key_another_user_owns_is_refused_by_load_and_by_rotate() {
    let dir = open_home();
    let home = Home::open(dir.path()).unwrap();
    let before = home.identity().unwrap().endpoint_id();
    drop(home);

    for refused in [
        load_or_create_with(dir.path(), not_ours),
        rotate_with(dir.path(), not_ours),
    ] {
        let Err(Error::Policy(message)) = refused else {
            panic!("a key another user owns was accepted");
        };
        assert!(message.contains("belongs to another user"), "{message}");
        assert!(message.contains("identity.key"), "{message}");
    }
    // Neither call changed the key, and it still loads when the owner check says it is this
    // user's.
    let home = Home::open(dir.path()).unwrap();
    assert_eq!(home.identity().unwrap().endpoint_id(), before);
    assert!(load_or_create_with(dir.path(), ours).is_ok());
}

#[test]
fn a_key_that_is_a_folder_is_not_a_regular_file() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    fs::create_dir(dir.path().join("identity.key")).unwrap();
    let Err(Error::Policy(message)) = home.identity() else {
        panic!("a folder as the key was accepted");
    };
    assert!(message.contains("not a regular file"), "{message}");
    // Rotating does not write over it either.
    let Err(Error::Policy(message)) = home.rotate_identity() else {
        panic!("rotate accepted a folder as the key");
    };
    assert!(message.contains("not a regular file"), "{message}");
    assert!(dir.path().join("identity.key").is_dir());
}

#[cfg(unix)]
#[test]
fn a_key_that_is_a_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    let elsewhere = dir.path().join("elsewhere.key");
    fs::write(&elsewhere, [7u8; 32]).unwrap();
    std::os::unix::fs::symlink(&elsewhere, dir.path().join("identity.key")).unwrap();
    let Err(Error::Policy(message)) = home.identity() else {
        panic!("a link at the key was accepted");
    };
    assert!(message.contains("is a link"), "{message}");
    let Err(Error::Policy(message)) = home.rotate_identity() else {
        panic!("rotate accepted a link at the key");
    };
    assert!(message.contains("is a link"), "{message}");
}

/// A hard link is a second name for the same file. Whoever can write through that name can change
/// this user's key, so a key with two names is refused even though its owner is this user.
#[cfg(unix)]
#[test]
fn a_key_with_two_names_is_refused_by_load_and_by_rotate() {
    let dir = open_home();
    let home = Home::open(dir.path()).unwrap();
    let before = home.identity().unwrap().endpoint_id();
    fs::hard_link(
        dir.path().join("identity.key"),
        dir.path().join("second-name"),
    )
    .unwrap();
    for refused in [home.identity(), home.rotate_identity()] {
        let Err(Error::Policy(message)) = refused else {
            panic!("a key with two names was accepted");
        };
        assert!(message.contains("2 links"), "{message}");
        assert!(message.contains("must have one"), "{message}");
    }
    drop(home);
    // The key is the key it was: neither call replaced it. Removing the second name makes it
    // readable again, and it still names the same endpoint.
    fs::remove_file(dir.path().join("second-name")).unwrap();
    let home = Home::open(dir.path()).unwrap();
    assert_eq!(home.identity().unwrap().endpoint_id(), before);
}

#[test]
fn a_home_with_no_key_makes_one_and_rotates_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    let first = home.identity().unwrap().endpoint_id();
    let second = home.rotate_identity().unwrap().endpoint_id();
    assert_ne!(first, second);
}

/// The path of the key is in every message, so the operator knows which file to fix.
#[test]
fn a_refused_key_is_named_by_its_path() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    fs::create_dir(dir.path().join("identity.key")).unwrap();
    let Err(Error::Policy(message)) = home.identity() else {
        panic!("a folder as the key was accepted");
    };
    assert!(
        message.contains(&dir.path().join("identity.key").display().to_string()),
        "{message}"
    );
}
