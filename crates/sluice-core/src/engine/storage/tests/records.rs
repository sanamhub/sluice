//! Tests for the store marker, the identity a folder is claimed by, and the names and paths a store
//! is installed under.

#[cfg(windows)]
use super::super::install;

#[test]
fn a_marker_says_which_names_were_created_and_whether_the_run_was_forced() {
    use super::super::{recorded, remember};
    let dir = tempfile::tempdir().unwrap();
    let names = std::collections::BTreeSet::from(["made".to_string()]);
    remember(dir.path(), "abc123", &names, true, "id-1", "all").unwrap();
    let found = recorded(dir.path()).unwrap().unwrap();
    let marker = found.owner;
    assert_eq!(marker.root, "abc123");
    assert_eq!(marker.names, names);
    assert!(marker.force);
    assert!(found.id.is_some(), "the folder's identity was not taken");

    // A marker from a run that created nothing and was not forced records exactly that, and a
    // folder with no marker at all is not a resume. A marker is written once, so the first one is
    // removed as a store folder that is claimed again would have it removed with the folder.
    std::fs::remove_file(dir.path().join(super::super::MARKER)).unwrap();
    remember(
        dir.path(),
        "abc123",
        &std::collections::BTreeSet::new(),
        false,
        "id-2",
        "all",
    )
    .unwrap();
    let found = recorded(dir.path()).unwrap().unwrap();
    let marker = found.owner;
    assert!(marker.names.is_empty());
    assert!(!marker.force);
    assert_eq!(marker.id.as_deref(), Some("id-2"));
    let empty = tempfile::tempdir().unwrap();
    assert!(recorded(empty.path()).unwrap().is_none());
}

/// A store folder is made private as it is created, not a moment later: another user who can write in
/// the output could otherwise read the marker, and with it copy a store this home would resume from.
#[test]
fn the_store_folder_is_private_from_its_first_moment() {
    use super::super::claim;
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    claim(&store).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&store).unwrap().permissions().mode() & 0o777,
            0o700,
            "the store folder was made with the umask"
        );
    }
    assert!(store.is_dir());
}

/// A marker is written with `create_new` and without following a link, so a `ticket` link planted in
/// the store folder is refused instead of being written through. Without the no-follow open, the
/// link would let another user read, and forge, a marker by pointing it at their own file.
#[cfg(unix)]
#[test]
fn a_link_in_place_of_the_marker_is_refused() {
    use super::super::remember;
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    std::fs::create_dir(&store).unwrap();
    let outside = root.path().join("outside");
    std::os::unix::fs::symlink(&outside, store.join(super::super::MARKER)).unwrap();
    let err = remember(
        &store,
        "abc123",
        &std::collections::BTreeSet::new(),
        false,
        "id-1",
        "all",
    )
    .unwrap_err();
    assert!(matches!(err, crate::Error::Storage(_)), "{err}");
    assert!(!outside.exists(), "the marker was written through the link");
}
/// The identity of a store is taken while its marker is read, from the handle that checked the
/// folder, so a folder renamed away and another put under the same name is told apart even when the
/// new one carries a marker that says the same thing. A compare against an identity taken later
/// would compare two readings of the replacement.
#[test]
fn the_identity_read_with_the_marker_is_the_folders_own() {
    use super::super::{identify, recorded, remember};
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    std::fs::create_dir(&store).unwrap();
    let none = std::collections::BTreeSet::new();
    remember(&store, "abc123", &none, false, "id-1", "all").unwrap();
    let found = recorded(&store).unwrap().unwrap();
    let id = found
        .id
        .expect("the folder has no identity on this platform");
    assert_eq!(identify(&store).unwrap(), Some(id));

    std::fs::rename(&store, root.path().join("gone")).unwrap();
    std::fs::create_dir(&store).unwrap();
    remember(&store, "abc123", &none, false, "id-1", "all").unwrap();
    assert_ne!(
        identify(&store).unwrap(),
        Some(id),
        "a replacement folder carries the identity of the one it replaced"
    );
}
/// A file system whose rename cannot refuse a name is installed with a link, and the link
/// refuses an existing target exactly as the rename would.
#[test]
fn the_link_fallback_installs_and_still_refuses_a_taken_name() {
    use super::super::install::link_no_replace;
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

/// A Windows device path is refused before it is turned into a verbatim path. Windows reads `/` as a
/// separator, so `//./pipe/name` names the same device as `\\.\pipe\name` and has to be recognised
/// before the separators are rewritten.
#[cfg(windows)]
#[test]
fn a_device_path_is_refused_with_either_separator() {
    use super::super::install::is_device_path;
    use std::path::Path;

    for device in [
        r"\\.\C:",
        r"\\.\pipe\name",
        "//./pipe/name",
        "//./C:",
        r"//.\C:",
    ] {
        assert!(is_device_path(Path::new(device)), "{device}");
    }
    for file in [r"\\?\C:\x", r"C:\x", r"\\server\share\x", "a/../b"] {
        assert!(!is_device_path(Path::new(file)), "{file}");
    }
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
