//! On Unix, `identity` refuses a key that other users can read.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sluice_core::{Error, Home};

/// A key file with the given mode. The bytes are a fixed test value and no real key.
fn key_with_mode(dir: &Path, mode: u32) {
    let key = dir.join("identity.key");
    fs::write(&key, [7u8; 32]).unwrap();
    fs::set_permissions(&key, fs::Permissions::from_mode(mode)).unwrap();
}

fn mode_of(dir: &Path) -> u32 {
    fs::metadata(dir.join("identity.key"))
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn a_key_that_group_or_other_can_reach_is_refused_and_left_as_it_is() {
    for mode in [0o644, 0o640, 0o604, 0o660] {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        key_with_mode(dir.path(), mode);
        let error = home.identity().unwrap_err();
        let Error::Policy(message) = &error else {
            panic!("mode {mode:04o}: expected Policy, got {error:?}");
        };
        let key = dir.path().join("identity.key");
        assert!(message.contains(&key.display().to_string()), "{message}");
        assert!(message.contains(&format!("{mode:04o}")), "{message}");
        assert!(message.contains("chmod 600"), "{message}");
        // Compared as booleans so that a failure does not print the key.
        assert!(fs::read(&key).unwrap() == [7u8; 32], "mode {mode:04o}");
        assert_eq!(mode_of(dir.path()), mode);
    }
}

#[test]
fn a_key_only_the_owner_can_reach_is_read() {
    let mut ids = Vec::new();
    for mode in [0o600, 0o400] {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(dir.path()).unwrap();
        key_with_mode(dir.path(), mode);
        ids.push(home.identity().unwrap().endpoint_id());
        assert_eq!(mode_of(dir.path()), mode);
    }
    // The same 32 bytes in both files, so the same identity.
    assert_eq!(ids[0], ids[1]);
}

#[test]
fn rotating_a_loose_key_gives_a_private_one_and_that_one_loads() {
    let dir = tempfile::tempdir().unwrap();
    let home = Home::open(dir.path()).unwrap();
    key_with_mode(dir.path(), 0o644);
    let rotated = home.rotate_identity().unwrap().endpoint_id();
    assert_eq!(mode_of(dir.path()), 0o600);
    assert_eq!(home.identity().unwrap().endpoint_id(), rotated);
}
