//! The owner of the key and of the folders of the home, and a key with a second name. `Home::open`
//! and `Home::identity` refuse each of them, and doctor reports the same refusal.

use std::fs;

use super::{empty_home, finding, ready_home};
use crate::home::doctor::{Check, Finding, Location, Name};
use crate::{Error, ErrorKind, Home};

/// The owner check on the key and on the folders of the home is the one `Home::open` runs, and
/// doctor reports the same refusal. The owner is a parameter, because a folder or a key of another
/// user cannot be made without being root.
#[test]
fn a_folder_or_a_key_another_user_owns_is_refused_by_open_and_reported_by_doctor() {
    use crate::home::doctor::checks::{home_with_owner, key_with_owner};

    let dir = ready_home();
    let location = Location {
        path: dir.path().to_path_buf(),
        from_env: false,
    };
    let blobs_not_ours = |path: &std::path::Path| Ok(!path.ends_with("blobs"));
    assert_eq!(
        home_with_owner(location.clone(), blobs_not_ours),
        Finding::HomeFolderOwnedByOther(location, "blobs")
    );
    let Err(Error::Policy(message)) =
        crate::home::setup::prepare_with(dir.path(), false, |_| Ok(()), blobs_not_ours)
    else {
        panic!("a blobs folder another user owns was opened");
    };
    assert!(message.contains("blobs"), "{message}");
    assert!(message.contains("belongs to another user"), "{message}");

    assert_eq!(
        key_with_owner(dir.path(), dir.path(), |_| Ok(false)),
        Finding::KeyOwnedByOther
    );
    let Err(Error::Policy(message)) =
        crate::home::identity::load_or_create_with(dir.path(), |_| Ok(false))
    else {
        panic!("a key another user owns was loaded");
    };
    assert!(message.contains("belongs to another user"), "{message}");
}

/// A hard link is a second name for the key, so it can be changed through that name. Doctor fails
/// it, and `Home::identity` refuses it: the same answer from each. A hard link needs no privilege
/// on NTFS, so this runs on every platform.
#[test]
fn a_key_with_two_names_is_a_policy_failure_and_open_agrees() {
    let dir = ready_home();
    fs::hard_link(
        dir.path().join("identity.key"),
        dir.path().join("second-name"),
    )
    .expect("hard link");
    let reported = finding(dir.path(), Name::Key);
    assert_eq!(reported, Finding::KeyHasLinks(2));
    assert_eq!(Check::new(reported).failure(), Some(ErrorKind::Policy));
    let Err(Error::Policy(message)) = Home::open(dir.path()).expect("open").identity() else {
        panic!("a key with two names loaded");
    };
    assert!(message.contains("2 links"), "{message}");
}

/// A key that is a link is not followed, and doctor says so rather than reading through it.
#[cfg(unix)]
#[test]
fn a_key_that_is_a_link_is_a_policy_failure() {
    let dir = ready_home();
    let key = dir.path().join("identity.key");
    let real = dir.path().join("real.key");
    fs::rename(&key, &real).expect("rename");
    std::os::unix::fs::symlink(&real, &key).expect("symlink");
    let reported = finding(dir.path(), Name::Key);
    assert_eq!(reported, Finding::KeyIsLink);
    assert_eq!(Check::new(reported).failure(), Some(ErrorKind::Policy));
    let Err(Error::Policy(message)) = Home::open(dir.path()).expect("open").identity() else {
        panic!("a link at the key loaded");
    };
    assert!(message.contains("is a link"), "{message}");
}

/// A key that is a junction is a link too. A junction is what a Windows user without the right to
/// make a symbolic link can still make, and the key is opened without following the reparse point,
/// so doctor, `Home::identity` and `rotate_identity` all refuse it as a link.
#[cfg(windows)]
#[test]
fn a_key_that_is_a_junction_is_a_policy_failure() {
    let dir = ready_home();
    let key = dir.path().join("identity.key");
    let elsewhere = dir.path().join("elsewhere");
    fs::create_dir(&elsewhere).expect("a folder for the junction");
    fs::remove_file(&key).expect("remove the key");
    let made = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&key)
        .arg(&elsewhere)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("mklink");
    assert!(made.success(), "mklink /J failed");
    let reported = finding(dir.path(), Name::Key);
    assert_eq!(reported, Finding::KeyIsLink);
    assert_eq!(Check::new(reported).failure(), Some(ErrorKind::Policy));
    let home = Home::open(dir.path()).expect("open");
    let Err(Error::Policy(message)) = home.identity() else {
        panic!("a junction at the key loaded");
    };
    assert!(message.contains("is a link"), "{message}");
    let Err(Error::Policy(message)) = home.rotate_identity() else {
        panic!("rotate accepted a junction at the key");
    };
    assert!(message.contains("is a link"), "{message}");
    assert!(dir.path().join("identity.key").is_dir(), "left as it was");
}

/// A key that is a folder is not a regular file, on every platform, and `Home::identity` refuses
/// it with the same kind of error.
#[test]
fn a_key_that_is_a_folder_is_not_a_regular_file() {
    let dir = empty_home();
    fs::create_dir(dir.path().join("identity.key")).expect("create_dir");
    let reported = finding(dir.path(), Name::Key);
    assert_eq!(reported, Finding::KeyNotAFile);
    assert_eq!(Check::new(reported).failure(), Some(ErrorKind::Policy));
    let Err(Error::Policy(message)) = Home::open(dir.path()).expect("open").identity() else {
        panic!("a folder as the key loaded");
    };
    assert!(message.contains("not a regular file"), "{message}");
}
