//! The form of a store's path that a record and a hold share.

use super::super::registry::canonical;

/// The path a record holds is the folder the store really lands in, so the same store is found
/// however the output was spelled: through `..` here, and through a link on Unix. A path whose
/// folder is not there has no resolved form, and the caller is told instead of given a second
/// spelling that would get a hold of its own.
#[test]
fn a_record_path_is_resolved_through_the_folder_that_holds_it() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let plain = out.join(".sluice-partial-x");
    let odd = out.join("..").join("out").join(".sluice-partial-x");
    assert_eq!(canonical(&plain).unwrap(), canonical(&odd).unwrap());

    #[cfg(unix)]
    {
        let link = dir.path().join("link-to-out");
        std::os::unix::fs::symlink(&out, &link).unwrap();
        assert_eq!(
            canonical(&link.join(".sluice-partial-x")).unwrap(),
            canonical(&plain).unwrap()
        );
    }

    let missing = dir.path().join("missing").join(".sluice-partial-x");
    let err = canonical(&missing).unwrap_err();
    assert!(matches!(err, crate::Error::Storage(_)), "{err}");
    assert!(err.to_string().contains("missing"), "{err}");
}

/// A link that does not lead anywhere is not a folder, even though the link itself is there: a path
/// under it has no resolved form, and the absolute spelling would be a key that a second spelling
/// of the same place could not meet.
#[cfg(unix)]
#[test]
fn a_path_under_a_dangling_link_has_no_key() {
    let dir = tempfile::tempdir().unwrap();
    let dangling = dir.path().join("dangling");
    std::os::unix::fs::symlink(dir.path().join("nowhere"), &dangling).unwrap();
    let err = canonical(&dangling.join(".sluice-partial-x")).unwrap_err();
    assert!(matches!(err, crate::Error::Storage(_)), "{err}");
    assert!(err.to_string().contains("dangling"), "{err}");
}

/// A volume that cannot resolve a folder still gives it one key: the verbatim form and the plain
/// form of an absolute path meet, whatever case the drive letter is written in, and so do the two
/// forms of a share.
#[cfg(windows)]
#[test]
fn a_volume_that_cannot_be_resolved_still_gives_one_key() {
    use std::path::PathBuf;

    use super::super::registry::one_form;

    for spelling in [r"C:\out", r"c:\out", r"\\?\C:\out", r"\\?\c:\out"] {
        assert_eq!(
            one_form(PathBuf::from(spelling)),
            PathBuf::from(r"c:\out"),
            "{spelling}"
        );
    }
    for spelling in [r"\\server\share\out", r"\\?\UNC\server\share\out"] {
        assert_eq!(
            one_form(PathBuf::from(spelling)),
            PathBuf::from(r"\\server\share\out"),
            "{spelling}"
        );
    }
}

/// Only `ERROR_INVALID_FUNCTION` is the failure the fallback exists for. A folder that is not there,
/// a link that loops and a name that is too long all fail for reasons of their own, and none of them
/// may be given the absolute spelling as a key.
#[cfg(windows)]
#[test]
fn only_the_one_windows_error_falls_back() {
    use super::super::registry::unusable_name;

    let raw = |code: i32| std::io::Error::from_raw_os_error(code);
    assert!(unusable_name(&raw(1)), "ERROR_INVALID_FUNCTION");
    for code in [2, 3, 5, 32, 123, 1004] {
        assert!(!unusable_name(&raw(code)), "{code}");
    }
    assert!(!unusable_name(&std::io::Error::from(
        std::io::ErrorKind::NotFound
    )));
}
