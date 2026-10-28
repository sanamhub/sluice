//! Tests for the disk side: the room a transfer needs, its store name, the two ways to
//! install a file without replacing one, and a path past the old Windows limit.

use super::{install, needed_bytes, partial_dir, temp_dir};
mod records;
mod store;

use std::path::Path;

#[test]
fn a_hundred_gigabytes_needs_about_a_hundred_and_not_two_hundred() {
    let need = needed_bytes(100 << 30, true);
    assert!(need > 100 << 30);
    assert!(need < 101 << 30, "{need}");
}

/// A volume that cannot link a finished file out of the store holds the whole content twice, so the
/// room it needs before a transfer starts is twice the content and not a little more than once.
#[test]
fn a_volume_without_links_needs_room_for_two_copies() {
    let linked = needed_bytes(100 << 30, true);
    let copied = needed_bytes(100 << 30, false);
    assert!(copied > 200 << 30, "{copied}");
    assert!(copied < 202 << 30, "{copied}");
    assert!(copied > linked * 2 - (4 << 20), "{copied} vs {linked}");
}

#[test]
fn the_partial_store_is_hidden_beside_the_output() {
    let root = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let path = partial_dir(Path::new("/out"), root);
    assert_eq!(path, Path::new(&format!("/out/.sluice-partial-{root}")));
}

#[test]
fn each_temporary_folder_has_its_own_name() {
    let dir = tempfile::tempdir().unwrap();
    let first = temp_dir(dir.path()).unwrap();
    let second = temp_dir(dir.path()).unwrap();
    assert_ne!(first.path(), second.path());
    assert!(first.path().is_dir() && second.path().is_dir());
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
    assert!(
        found.path.ends_with(".sluice-partial-0123456789abcdef"),
        "{}",
        found.path.display()
    );
    assert_eq!(found.root, "root-one");
    assert!(super::find_record(&store, "root-two").unwrap().is_none());
    super::forget_record(&store);
    assert!(super::find_record(&store, "root-one").unwrap().is_none());
    assert!(store.is_dir(), "a forgotten record removed the store");
}

/// The store's name is the whole root, so two transfers that agree in the first 16 characters of
/// their root cannot be handed one folder.
#[test]
fn the_store_name_carries_the_whole_root() {
    let out = Path::new("out");
    let one = format!("{}1", "a".repeat(63));
    let two = format!("{}2", "a".repeat(63));
    assert_ne!(partial_dir(out, &one), partial_dir(out, &two));
    let name = partial_dir(out, &one)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(name.ends_with(&one), "{name}");
}

/// The path a record holds is the folder the store really lands in, so the same store is found
/// however the output was spelled: through `..` here, and through a link on Unix.
#[test]
fn a_record_path_is_resolved_through_the_folder_that_holds_it() {
    use super::registry::canonical;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let plain = out.join(".sluice-partial-x");
    let odd = out.join("..").join("out").join(".sluice-partial-x");
    assert_eq!(canonical(&plain), canonical(&odd));

    #[cfg(unix)]
    {
        let link = dir.path().join("link-to-out");
        std::os::unix::fs::symlink(&out, &link).unwrap();
        assert_eq!(
            canonical(&link.join(".sluice-partial-x")),
            canonical(&plain)
        );
    }
}

/// A store that is a link would put this receive's bytes wherever the link leads, and its marker
/// is not read at all: the folder is refused before anything in it is opened.
#[cfg(unix)]
#[test]
fn a_store_that_is_a_link_is_never_read() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join(super::MARKER), "root\nid x\n").unwrap();
    let store = dir.path().join(".sluice-partial-x");
    std::os::unix::fs::symlink(&elsewhere, &store).unwrap();
    let err = super::recorded(&store).unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    assert!(store.is_symlink(), "the link was removed");
}

/// `--force` on a target that is a link replaces the link and touches nothing the link points at: a
/// link in the output must not let a receive change a file outside it.
#[test]
fn replacing_a_link_never_changes_what_it_points_at() {
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.bin");
    std::fs::write(&outside, b"the user's file").unwrap();
    let target = dir.path().join("target.bin");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::os::unix::fs::symlink(&outside, &target).unwrap();
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&outside, &target).is_err() {
            eprintln!("SKIPPED: this account cannot create a symlink on Windows");
            return;
        }
        let mut permissions = std::fs::metadata(&outside).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&outside, permissions).unwrap();
    }
    #[cfg(not(any(unix, windows)))]
    return;
    let incoming = dir.path().join("incoming");
    std::fs::write(&incoming, b"the new file").unwrap();
    install(&incoming, &target, true).unwrap();
    assert!(
        !std::fs::symlink_metadata(&target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"the new file");
    assert_eq!(std::fs::read(&outside).unwrap(), b"the user's file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o444
        );
    }
    #[cfg(windows)]
    assert!(
        std::fs::metadata(&outside)
            .unwrap()
            .permissions()
            .readonly()
    );
}

/// Clearing a read-only flag is done through a handle that does not follow a link, so a link in the
/// output cannot have a file outside it changed.
#[cfg(unix)]
#[test]
fn clearing_a_read_only_flag_never_follows_a_link() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside.bin");
    std::fs::write(&outside, b"the user's file").unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o444)).unwrap();
    let link = dir.path().join("link.bin");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(super::install::clear_readonly(&link).is_err());
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

/// A rename that cannot refuse a name is recognized by its error number, which macOS and Linux
/// spell differently. Every number either uses means "this file system has no such flag".
#[cfg(unix)]
#[test]
fn the_errnos_that_mean_a_rename_flag_is_unsupported_are_known() {
    use super::install::unsupported;
    for errno in [22, 38, 45, 78, 95, 102] {
        assert!(
            unsupported(&std::io::Error::from_raw_os_error(errno)),
            "{errno}"
        );
    }
    assert!(!unsupported(&std::io::Error::from_raw_os_error(17)));
}

/// A device path names a device, not a file, and building a verbatim path from one names something
/// else entirely. Every prefix that reaches a device namespace is refused, with either separator,
/// and a verbatim file path is still a file path.
#[cfg(windows)]
#[test]
fn a_device_path_is_not_a_file_path() {
    use std::path::Path;

    for device in [
        r"\\.\C:\x",
        r"\\.\pipe\name",
        "//./pipe/name",
        r"\??\C:\x",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1",
        "//?/GLOBALROOT/Device/HarddiskVolume1",
        r"\\?\Volume{00000000-0000-0000-0000-000000000000}\x",
        r"\\?\",
    ] {
        assert!(
            super::install::is_device_path(Path::new(device)),
            "{device}"
        );
    }
    for file in [
        r"C:\out\file",
        r"\\?\C:\x",
        "//?/c:/x",
        r"\\?\UNC\server\share\x",
        r"\\server\share\x",
    ] {
        assert!(!super::install::is_device_path(Path::new(file)), "{file}");
    }

    let dir = tempfile::tempdir().unwrap();
    let incoming = dir.path().join("incoming");
    std::fs::write(&incoming, b"x").unwrap();
    let err = install(&incoming, Path::new(r"\\.\C:\nope"), true).unwrap_err();
    assert!(err.to_string().contains("device path"), "{err}");
}

/// A store folder is made for this user alone: another user who can write in the output cannot read
/// the marker through it, and cannot copy a store in.
#[test]
fn the_store_folder_is_private() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".sluice-partial-x");
    super::claim(&store).unwrap();
    assert!(store.is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&store).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
}

/// A store is made with one `create_dir`, not a recursive create. A path whose parent is missing is
/// refused, so a folder another process left somewhere above the store is never taken for its
/// parent and no folder is created where the caller did not prepare one.
#[test]
fn the_store_folder_is_made_with_one_create() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("missing").join(".sluice-partial-x");
    let err = super::claim(&store).unwrap_err();
    assert!(matches!(err, crate::Error::Storage(_)), "{err}");
    assert!(!dir.path().join("missing").exists(), "the parent was made");
}

/// A store is never claimed over a folder that is already there: when the folder cannot be emptied,
/// the name is taken by the time the store would be made, and what holds it is neither removed nor
/// written into.
#[test]
fn the_store_folder_is_never_claimed_over_a_folder_it_cannot_empty() {
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join(".sluice-partial-y");
    std::fs::create_dir_all(blocked.join("keep")).unwrap();
    // A name inside that this process cannot remove, so emptying the folder fails.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if rustix::process::geteuid().as_raw() == 0 {
            eprintln!("SKIPPED: root can empty any folder");
            return;
        }
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o500)).unwrap();
    }
    // A file held open for reading only cannot be deleted through its name, so the folder stays.
    #[cfg(windows)]
    let _held = {
        use std::os::windows::fs::OpenOptionsExt;
        let file = blocked.join("keep").join("held");
        std::fs::write(&file, b"held").unwrap();
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&file)
            .unwrap()
    };
    #[cfg(not(any(unix, windows)))]
    return;
    let err = super::claim(&blocked).unwrap_err();
    assert!(matches!(err, crate::Error::Policy(_)), "{err}");
    assert!(blocked.exists(), "a folder it did not make was changed");
}
