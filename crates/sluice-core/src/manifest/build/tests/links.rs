//! Symlinks and special files. A link is recorded and never followed, and one whose target a
//! receiver would reject is skipped and listed while the rest of the folder is built. Making a
//! symlink on Windows needs a privilege, so those tests say when they skip.

use super::common::*;
use crate::manifest::{Kind, Loc, Reason};

#[test]
fn a_symlink_is_recorded_with_its_target_and_never_followed() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "data/inner.txt", b"inside");
    put(dir.path(), "real.txt", b"real");
    if !symlink("real.txt", &dir.path().join("link.txt")) {
        return;
    }
    // A link to a folder is not entered either: nothing under `data` shows up twice.
    let folder_link = dir.path().join("data-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink("data", &folder_link).expect("a symlink");
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir("data", &folder_link).expect("a symlink");

    let built = ready(dir.path());
    assert_eq!(
        listing(&built),
        [
            ("data", Kind::Dir),
            ("data-link", Kind::Symlink),
            ("data/inner.txt", Kind::File),
            ("link.txt", Kind::Symlink),
            ("real.txt", Kind::File),
        ]
    );
    let link = built
        .manifest
        .entries()
        .iter()
        .find(|e| e.path.as_str() == "link.txt")
        .expect("the entry");
    assert_eq!(
        link.link_target.as_ref().map(|t| t.as_str()),
        Some("real.txt")
    );
    assert_eq!((link.size, link.mode, link.loc), (0, 0, Loc::None));
    assert!(built.skipped.is_empty());
}

#[test]
fn a_link_that_points_inside_through_dotdot_is_accepted() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "b", b"x");
    mkdir(dir.path(), "a");
    // On Windows this is written `..\b`, which is the same target with the separator it has.
    let target = if cfg!(windows) { "..\\b" } else { "../b" };
    if !symlink(target, &dir.path().join("a").join("link")) {
        return;
    }
    let built = ready(dir.path());
    let link = built
        .manifest
        .entries()
        .iter()
        .find(|e| e.path.as_str() == "a/link")
        .expect("the entry");
    assert_eq!(link.link_target.as_ref().map(|t| t.as_str()), Some("../b"));
    assert!(built.skipped.is_empty());
}

#[test]
fn a_link_that_steps_above_the_folder_is_skipped_and_listed_and_the_rest_is_built() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "kept.txt", b"x");
    let target = if cfg!(windows) {
        "..\\outside"
    } else {
        "../outside"
    };
    if !symlink(target, &dir.path().join("escape")) {
        return;
    }
    let built = ready(dir.path());
    assert_eq!(listing(&built), [("kept.txt", Kind::File)]);
    assert_eq!(built.skipped, [finding("escape", Reason::LinkTarget(5))]);
}

#[test]
fn an_absolute_target_is_skipped_and_listed() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "kept.txt", b"x");
    // `/` begins an absolute target on Unix (rule 2). A Windows drive path has a colon (rule 4).
    let (target, rule) = if cfg!(windows) {
        ("C:\\Windows\\notepad.exe", 4)
    } else {
        ("/etc/passwd", 2)
    };
    if !symlink(target, &dir.path().join("absolute")) {
        return;
    }
    let built = ready(dir.path());
    assert_eq!(listing(&built), [("kept.txt", Kind::File)]);
    assert_eq!(
        built.skipped,
        [finding("absolute", Reason::LinkTarget(rule))]
    );
}

#[test]
fn skipped_links_are_listed_even_when_the_build_is_refused() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "bad\u{202e}", b"x");
    let target = if cfg!(windows) { "C:\\x" } else { "/x" };
    if !symlink(target, &dir.path().join("absolute")) {
        return;
    }
    let refusal = refused(dir.path());
    assert_eq!(refusal.problems.len(), 1);
    assert_eq!(refusal.skipped.len(), 1);
}

#[test]
fn the_folder_that_was_sent_is_followed_when_it_is_itself_a_link() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "real/file.txt", b"x");
    let link = dir.path().join("sent");
    #[cfg(unix)]
    std::os::unix::fs::symlink("real", &link).expect("a symlink");
    #[cfg(windows)]
    if let Err(e) = std::os::windows::fs::symlink_dir("real", &link) {
        assert!(
            !std::env::var("SLUICE_REQUIRE_SYMLINKS").is_ok_and(|v| !v.is_empty()),
            "SLUICE_REQUIRE_SYMLINKS is set, and this account cannot make a symlink: {e}"
        );
        eprintln!("SKIPPED: this account cannot create a symlink on Windows ({e})");
        return;
    }
    let built = ready(&link);
    assert_eq!(listing(&built), [("file.txt", Kind::File)]);
}

#[test]
fn a_symlink_with_a_bad_name_refuses_the_build_whatever_it_points_at() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "target.txt", b"x");
    if !symlink("target.txt", &dir.path().join("link\u{202e}")) {
        return;
    }
    let refusal = refused(dir.path());
    assert_eq!(
        refusal.problems,
        [finding("link\u{202e}", Reason::PathRule(6))]
    );
    assert!(refusal.skipped.is_empty());
}

#[cfg(unix)]
mod unix {
    use std::ffi::OsStr;
    use std::fs;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[test]
    fn a_target_with_an_empty_component_or_a_backslash_is_skipped() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "kept.txt", b"x");
        symlink("a//b", dir.path().join("empty")).expect("a symlink");
        symlink("a\\b", dir.path().join("backslash")).expect("a symlink");
        symlink("con", dir.path().join("device")).expect("a symlink");
        let built = ready(dir.path());
        assert_eq!(
            built.skipped,
            [
                finding("backslash", Reason::LinkTarget(4)),
                finding("device", Reason::LinkTarget(4)),
                finding("empty", Reason::LinkTarget(3)),
            ]
        );
    }

    #[test]
    fn a_target_that_is_not_utf8_is_skipped_and_listed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "kept.txt", b"x");
        symlink(OsStr::from_bytes(b"to-\xff"), dir.path().join("odd")).expect("a symlink");
        let built = ready(dir.path());
        assert_eq!(built.skipped, [finding("odd", Reason::LinkTargetNotUtf8)]);
        assert_eq!(listing(&built), [("kept.txt", Kind::File)]);
    }

    #[test]
    fn a_dangling_link_is_recorded_like_any_other() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        symlink("nowhere", dir.path().join("dangling")).expect("a symlink");
        let built = ready(dir.path());
        assert_eq!(listing(&built), [("dangling", Kind::Symlink)]);
    }

    #[test]
    fn a_socket_is_skipped_and_listed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "kept.txt", b"x");
        let _socket =
            std::os::unix::net::UnixListener::bind(dir.path().join("sock")).expect("a socket");
        let built = ready(dir.path());
        assert_eq!(built.skipped, [finding("sock", Reason::NotAFileOrFolder)]);
        assert_eq!(listing(&built), [("kept.txt", Kind::File)]);
    }

    #[test]
    fn a_fifo_is_skipped_and_listed_and_never_opened() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "kept.txt", b"x");
        let made = std::process::Command::new("mkfifo")
            .arg(dir.path().join("pipe"))
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("SKIPPED: mkfifo is not available");
            return;
        }
        // Opening a FIFO for reading blocks until a writer comes, so a build that opened this
        // one would hang here and not fail.
        let built = ready(dir.path());
        assert_eq!(built.skipped, [finding("pipe", Reason::NotAFileOrFolder)]);
    }

    #[test]
    fn a_file_that_cannot_be_read_refuses_the_build_and_is_listed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "locked.txt", b"x");
        put(dir.path(), "open.txt", b"y");
        let path = dir.path().join("locked.txt");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::File::open(&path).is_ok() {
            eprintln!("SKIPPED: this account reads files whatever their mode (root)");
            return;
        }
        let refusal = refused(dir.path());
        assert_eq!(refusal.problems.len(), 1);
        assert_eq!(refusal.problems[0].path, "locked.txt");
        assert!(matches!(refusal.problems[0].reason, Reason::Unreadable(_)));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("chmod back");
    }

    #[test]
    fn a_folder_that_cannot_be_read_refuses_the_build_and_is_listed() {
        let dir = tempfile::tempdir().expect("a temporary folder");
        put(dir.path(), "closed/inner.txt", b"x");
        let path = dir.path().join("closed");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).expect("chmod");
        if fs::read_dir(&path).is_ok() {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod back");
            eprintln!("SKIPPED: this account lists folders whatever their mode (root)");
            return;
        }
        let refusal = refused(dir.path());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod back");
        assert_eq!(refusal.problems.len(), 1);
        assert_eq!(refusal.problems[0].path, "closed");
        assert!(matches!(refusal.problems[0].reason, Reason::Unreadable(_)));
    }
}
