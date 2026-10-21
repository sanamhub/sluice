//! What the build records for files and folders: kinds, sizes, roots, times, modes, and the check
//! that a file did not change while it was read.

use std::fs::{self, File, OpenOptions};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::common::*;
use crate::Error;
use crate::manifest::build::hash::{HashFault, hash_file_with};
use crate::manifest::{Build, Kind, Limits, Loc, Manifest, build::build};

#[test]
fn every_folder_has_an_entry_and_the_folder_that_was_sent_has_none() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "docs/readme.txt", b"hello");
    mkdir(dir.path(), "empty");
    mkdir(dir.path(), "a/b/c");
    put(dir.path(), "top.bin", &[1, 2, 3]);

    let built = ready(dir.path());
    assert_eq!(
        listing(&built),
        [
            ("a", Kind::Dir),
            ("a/b", Kind::Dir),
            ("a/b/c", Kind::Dir),
            ("docs", Kind::Dir),
            ("docs/readme.txt", Kind::File),
            ("empty", Kind::Dir),
            ("top.bin", Kind::File),
        ]
    );
    assert!(built.skipped.is_empty());
}

#[test]
fn a_folder_with_nothing_in_it_is_the_empty_manifest() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let built = ready(dir.path());
    assert_eq!(built.manifest.encode(), [1, 0, 0]);
}

#[test]
fn hidden_files_are_included_and_there_are_no_ignore_rules() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), ".hidden", b"x");
    put(dir.path(), ".git/config", b"y");
    put(dir.path(), ".gitignore", b"*");
    put(dir.path(), "target/out", b"z");
    let built = ready(dir.path());
    let paths: Vec<&str> = listing(&built).iter().map(|(p, _)| *p).collect();
    assert_eq!(
        paths,
        [
            ".git",
            ".git/config",
            ".gitignore",
            ".hidden",
            "target",
            "target/out"
        ]
    );
}

#[test]
fn the_root_of_a_file_is_the_blake3_hash_of_its_content() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    // Three chunk groups and a bit: more than one read of the hasher's buffer.
    let big: Vec<u8> = (0..3_200_000u32).map(|n| (n % 251) as u8).collect();
    put(dir.path(), "big", &big);
    put(dir.path(), "small", b"hello");
    put(dir.path(), "empty", b"");

    let built = ready(dir.path());
    let entries = built.manifest.entries();
    let root_of = |name: &str| match entries.iter().find(|e| e.path.as_str() == name) {
        Some(e) => (e.size, e.loc),
        None => panic!("no entry {name}"),
    };
    let blob = |bytes: &[u8]| Loc::Blob {
        root: *blake3::hash(bytes).as_bytes(),
    };
    assert_eq!(root_of("big"), (3_200_000, blob(&big)));
    assert_eq!(root_of("small"), (5, blob(b"hello")));
    assert_eq!(root_of("empty"), (0, blob(b"")));
}

#[test]
fn the_manifest_that_was_built_is_one_a_receiver_accepts() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a/b.txt", b"1");
    put(dir.path(), "a/c/d.txt", b"22");
    put(dir.path(), "é/日本語.txt", b"333");
    let built = ready(dir.path());
    let bytes = built.manifest.encode();
    let decoded = Manifest::decode(&bytes, Limits::default()).expect("a receiver accepts it");
    assert_eq!(decoded, built.manifest);
    // Same folder, same bytes, same root.
    let again = ready(dir.path());
    assert_eq!(again.manifest.entries(), built.manifest.entries());
    assert_eq!(again.manifest.root(), built.manifest.root());
}

#[test]
fn a_hard_link_is_a_separate_file() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "one", b"same bytes");
    fs::hard_link(dir.path().join("one"), dir.path().join("two")).expect("a hard link");
    let built = ready(dir.path());
    let entries = built.manifest.entries();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().all(|e| e.kind == Kind::File && e.size == 10));
    assert_eq!(entries[0].loc, entries[1].loc);
}

#[test]
fn a_time_before_1970_is_written_as_0_and_a_later_one_in_milliseconds() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "old", b"x");
    put(dir.path(), "new", b"y");
    let set = |name: &str, time: SystemTime| {
        let file = OpenOptions::new()
            .write(true)
            .open(dir.path().join(name))
            .expect("open to set the time");
        file.set_modified(time).is_ok()
    };
    let old_ok = set("old", UNIX_EPOCH - Duration::from_secs(86_400 * 365));
    assert!(set(
        "new",
        UNIX_EPOCH + Duration::from_millis(1_800_000_000_123)
    ));
    let built = ready(dir.path());
    let time_of = |name: &str| {
        built
            .manifest
            .entries()
            .iter()
            .find(|e| e.path.as_str() == name)
            .map(|e| e.mtime_ms)
    };
    assert_eq!(time_of("new"), Some(1_800_000_000_123));
    if old_ok {
        assert_eq!(time_of("old"), Some(0));
    } else {
        eprintln!("SKIPPED: this filesystem cannot hold a time before 1970");
    }
}

#[cfg(unix)]
#[test]
fn modes_are_the_permission_bits_of_the_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "plain", b"x");
    put(dir.path(), "run", b"x");
    put(dir.path(), "secret", b"x");
    put(dir.path(), "setuid", b"x");
    mkdir(dir.path(), "closed");
    let set = |name: &str, mode: u32| {
        fs::set_permissions(dir.path().join(name), fs::Permissions::from_mode(mode))
            .expect("set the mode");
    };
    set("plain", 0o644);
    set("run", 0o755);
    set("secret", 0o600);
    // Recorded as it is on disk. A receiver is the one that never applies it.
    set("setuid", 0o4755);
    set("closed", 0o700);
    let built = ready(dir.path());
    let mode_of = |name: &str| {
        built
            .manifest
            .entries()
            .iter()
            .find(|e| e.path.as_str() == name)
            .map(|e| e.mode)
    };
    assert_eq!(mode_of("plain"), Some(0o644));
    assert_eq!(mode_of("run"), Some(0o755));
    assert_eq!(mode_of("secret"), Some(0o600));
    assert_eq!(mode_of("setuid"), Some(0o4755));
    assert_eq!(mode_of("closed"), Some(0o700));
    // Every mode is within what a receiver accepts: no file type bits.
    assert!(built.manifest.entries().iter().all(|e| e.mode <= 0o7777));
}

#[cfg(windows)]
#[test]
// On Windows `set_readonly(false)` clears the one attribute it names, and nothing else.
#[allow(clippy::permissions_set_readonly_false)]
fn windows_writes_0o444_for_a_read_only_file_0o644_for_another_and_0o755_for_a_folder() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "locked", b"x");
    put(dir.path(), "open", b"x");
    mkdir(dir.path(), "folder");
    let path = dir.path().join("locked");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).expect("set read-only");

    let built = ready(dir.path());
    let mode_of = |name: &str| {
        built
            .manifest
            .entries()
            .iter()
            .find(|e| e.path.as_str() == name)
            .map(|e| e.mode)
    };
    assert_eq!(mode_of("locked"), Some(0o444));
    assert_eq!(mode_of("open"), Some(0o644));
    assert_eq!(mode_of("folder"), Some(0o755));

    // Let the temporary folder be removed.
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).expect("clear read-only");
}

#[test]
fn a_file_that_grew_while_it_was_read_fails_the_build_without_naming_it() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "secret-name.txt", b"hello");
    let result = build(dir.path(), |path| {
        hash_file_with(path, || {
            let mut file = OpenOptions::new().append(true).open(path).expect("open");
            std::io::Write::write_all(&mut file, b" and more").expect("append");
        })
    });
    let Err(Error::Storage(message)) = result else {
        panic!("expected a storage error");
    };
    assert!(message.contains("changed while it was read"), "{message}");
    assert!(!message.contains("secret"), "{message}");
}

#[test]
fn a_file_whose_time_changed_while_it_was_read_fails_the_build() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a.txt", b"hello");
    let result = build(dir.path(), |path| {
        hash_file_with(path, || {
            let file = OpenOptions::new().write(true).open(path).expect("open");
            let later = SystemTime::now() + Duration::from_secs(3600);
            file.set_modified(later).expect("set the time");
        })
    });
    assert!(matches!(result, Err(Error::Storage(_))));
}

#[test]
fn a_file_that_shrank_while_it_was_read_fails_the_build() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a.txt", b"hello world");
    let result = build(dir.path(), |path| {
        hash_file_with(path, || {
            File::create(path).expect("truncate");
        })
    });
    assert!(matches!(result, Err(Error::Storage(_))));
}

#[test]
fn a_file_that_does_not_change_is_hashed_with_its_size_and_time_from_the_same_look() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a.txt", b"hello");
    let path = dir.path().join("a.txt");
    let hashed = hash_file_with(&path, || {}).expect("an unchanged file hashes");
    assert_eq!(hashed.size, 5);
    assert_eq!(hashed.root, *blake3::hash(b"hello").as_bytes());
    assert!(matches!(
        hash_file_with(&dir.path().join("missing"), || {}),
        Err(HashFault::Unreadable(_))
    ));
}

#[test]
fn a_root_that_is_not_a_folder_or_does_not_exist_is_an_error_without_the_path() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "plain.txt", b"x");
    let err = Manifest::build(&dir.path().join("plain.txt")).unwrap_err();
    assert!(matches!(err, Error::Policy(_)), "{err}");
    assert!(!err.to_string().contains("plain"), "{err}");
    let err = Manifest::build(&dir.path().join("missing-folder")).unwrap_err();
    assert!(matches!(err, Error::Storage(_)), "{err}");
    assert!(!err.to_string().contains("missing-folder"), "{err}");
}

#[test]
fn a_deep_tree_does_not_use_the_stack() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let deep = vec!["d"; 300].join("/");
    mkdir(dir.path(), &deep);
    let root = dir.path().to_path_buf();
    // A walk that recursed would need a frame per folder, and 64 KiB holds far fewer than 300.
    let found = std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(
            move || match Manifest::build(&root).expect("the build runs") {
                Build::Ready(built) => built.manifest.entries().len(),
                Build::Refused(refusal) => panic!("refused: {}", refusal.problems.len()),
            },
        )
        .expect("a thread")
        .join()
        .expect("the walk did not overflow");
    assert_eq!(found, 300);
}
