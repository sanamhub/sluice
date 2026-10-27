//! Tests for the disk side: the room a transfer needs, its store name, the two ways to
//! install a file without replacing one, and a path past the old Windows limit.

use super::{install, needed_bytes, partial_dir, temp_dir};
use std::path::Path;

#[test]
fn a_hundred_gigabytes_needs_about_a_hundred_and_not_two_hundred() {
    let need = needed_bytes(100 << 30);
    assert!(need > 100 << 30);
    assert!(need < 101 << 30, "{need}");
}

#[test]
fn the_partial_store_is_hidden_beside_the_output() {
    let path = partial_dir(Path::new("/out"), "0123456789abcdef0123");
    assert_eq!(path, Path::new("/out/.sluice-partial-0123456789abcdef"));
}

#[test]
fn each_temporary_folder_has_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let first = temp_dir(dir.path()).unwrap();
    let second = temp_dir(dir.path()).unwrap();
    assert_ne!(first, second);
    assert!(first.is_dir() && second.is_dir());
}

#[test]
fn an_install_without_force_leaves_the_target_alone() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let incoming = dir.path().join("incoming");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&incoming, b"new").unwrap();
    let err = install(&incoming, &target, false).unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    assert_eq!(std::fs::read(&target).unwrap(), b"old");
    assert!(incoming.exists());
}

#[test]
fn a_file_that_appears_after_the_check_is_not_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("late");
    let incoming = dir.path().join("incoming");
    std::fs::write(&incoming, b"new").unwrap();
    // The plan ran while the target was absent; another process made it since.
    std::fs::write(&target, b"theirs").unwrap();
    assert!(install(&incoming, &target, false).is_err());
    assert_eq!(std::fs::read(&target).unwrap(), b"theirs");
}

#[test]
fn an_install_with_force_replaces_the_target() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target");
    let incoming = dir.path().join("incoming");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&incoming, b"new").unwrap();
    install(&incoming, &target, true).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!incoming.exists());
}

/// A file system whose rename cannot refuse a name is installed with a link, and the link
/// refuses an existing target exactly as the rename would.
#[test]
fn the_link_fallback_installs_and_still_refuses_a_taken_name() {
    use super::link_no_replace;
    let dir = tempfile::tempdir().unwrap();
    let incoming = dir.path().join("incoming");
    let target = dir.path().join("target");
    std::fs::write(&incoming, b"new").unwrap();
    link_no_replace(&incoming, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!incoming.exists());
    std::fs::write(&incoming, b"another").unwrap();
    let err = link_no_replace(&incoming, &target).unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}

/// `MoveFileExW` needs the verbatim form for a path past 260 characters, which `std::fs` builds
/// itself. Without it the last step of a deep transfer is the one that fails.
#[cfg(windows)]
#[test]
fn a_path_past_two_hundred_and_sixty_characters_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let mut deep = dir.path().to_path_buf();
    for _ in 0..12 {
        deep.push("a-folder-name-long-enough");
    }
    std::fs::create_dir_all(&deep).unwrap();
    let target = deep.join("installed.bin");
    assert!(target.as_os_str().len() > 260, "{}", target.display());
    let incoming = dir.path().join("incoming");
    std::fs::write(&incoming, b"bytes").unwrap();
    install(&incoming, &target, false).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"bytes");
}
