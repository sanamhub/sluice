//! The disk path behind each file entry, which can differ from the entry's path.

use std::ffi::OsStr;
use std::fs;

use super::common::*;
use crate::manifest::Kind;

#[test]
fn a_file_with_an_nfd_name_is_found_by_its_disk_path_and_listed_by_its_nfc_path() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    // `e` and a combining acute, which NFC makes U+00E9. The disk keeps the bytes it was given.
    put(dir.path(), "cafe\u{301}/note\u{301}.txt", b"inside");
    put(dir.path(), "a.txt", b"first");
    put(dir.path(), "z.txt", b"last");

    let built = ready(dir.path());
    assert_eq!(
        listing(&built),
        [
            ("a.txt", Kind::File),
            ("caf\u{e9}", Kind::Dir),
            ("caf\u{e9}/not\u{e9}.txt", Kind::File),
            ("z.txt", Kind::File),
        ]
    );
    let disk = built.disk.file(2).expect("a file entry has a disk path");
    assert_eq!(disk.file_name(), Some(OsStr::new("note\u{301}.txt")));
    assert_eq!(
        disk.parent().and_then(|folder| folder.file_name()),
        Some(OsStr::new("cafe\u{301}"))
    );
    assert_eq!(fs::read(disk).expect("read by its disk path"), b"inside");
}

#[test]
fn each_disk_path_is_at_the_index_of_its_entry_and_only_files_have_one() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    // Written in an order the walk is unlikely to return, with folders between the files.
    for name in ["m/2", "b", "m/1", "a/z", "q", "a/b/c"] {
        put(dir.path(), name, name.as_bytes());
    }
    let built = ready(dir.path());
    let entries = built.manifest.entries();
    for (index, entry) in entries.iter().enumerate() {
        match (entry.kind, built.disk.file(index)) {
            (Kind::File, Some(disk)) => {
                let content = fs::read(disk).expect("read by its disk path");
                assert_eq!(content, entry.path.as_str().as_bytes());
            }
            (Kind::Dir, None) => {}
            (kind, disk) => panic!("{kind:?} at {index} has disk path {disk:?}"),
        }
    }
    assert_eq!(built.disk.file(entries.len()), None);
}

#[test]
fn the_debug_form_counts_the_files_and_names_none() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "private-name.txt", b"x");
    mkdir(dir.path(), "folder");
    let built = ready(dir.path());
    let shown = format!("{:?}", built.disk);
    assert_eq!(shown, "DiskPaths { files: 1 }");
}
