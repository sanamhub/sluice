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

/// The store that holds the manifest until the checks pass lives in a private folder of the home,
/// not in the shared system temporary folder where another local user could read the names and
/// sizes of a transfer. It removes itself when the receive ends.
#[tokio::test]
async fn the_metadata_store_lives_in_the_private_folder_it_is_given() {
    use super::MetaStore;
    let parent = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(
            std::fs::metadata(parent.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
    let store = MetaStore::open_in(parent.path()).await.unwrap();
    let dir = store.dir().to_path_buf();
    assert_eq!(dir.parent(), Some(parent.path()));
    assert!(dir.is_dir());
    store.close().await.unwrap();
    assert!(!dir.exists(), "the store folder was left behind");
}
/// A marker says which names the run created and whether it was forced, so a resume knows what it
/// may write into and whether the run it continues was allowed to replace what was there.
#[test]
fn a_marker_says_which_names_were_created_and_whether_the_run_was_forced() {
    use super::{recorded, remember};
    let dir = tempfile::tempdir().unwrap();
    let names = std::collections::BTreeSet::from(["made".to_string()]);
    remember(dir.path(), "abc123", &names, true, "id-1").unwrap();
    let found = recorded(dir.path()).unwrap().unwrap();
    assert_eq!(found.root, "abc123");
    assert_eq!(found.names, names);
    assert!(found.force);

    // A marker from a run that created nothing and was not forced records exactly that, and a
    // folder with no marker at all is not a resume.
    remember(
        dir.path(),
        "abc123",
        &std::collections::BTreeSet::new(),
        false,
        "id-2",
    )
    .unwrap();
    let found = recorded(dir.path()).unwrap().unwrap();
    assert!(found.names.is_empty());
    assert!(!found.force);
    assert_eq!(found.id.as_deref(), Some("id-2"));
    let empty = tempfile::tempdir().unwrap();
    assert!(recorded(empty.path()).unwrap().is_none());
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

/// The home's record of a partial store is what makes a folder with the store's name trusted. A
/// record names one absolute path and one root, a record for another root is not this store's, and
/// forgetting the record leaves the folder alone.
#[test]
fn a_record_names_one_store_and_one_root() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".sluice-partial-0123456789abcdef");
    std::fs::create_dir(&store).unwrap();
    let id = super::remember_record(&store, "root-one").unwrap();
    let found = super::find_record(&store, "root-one").unwrap().unwrap();
    assert_eq!(found.id, id);
    assert_eq!(found.path, std::path::absolute(&store).unwrap());
    assert_eq!(found.root, "root-one");
    assert!(super::find_record(&store, "root-two").unwrap().is_none());
    super::forget_record(&store);
    assert!(super::find_record(&store, "root-one").unwrap().is_none());
    assert!(store.is_dir(), "a forgotten record removed the store");
}
