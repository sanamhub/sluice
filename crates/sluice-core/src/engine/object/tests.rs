use super::*;

#[test]
fn a_path_names_the_object_it_names_and_not_another() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("a")).unwrap();
    std::fs::create_dir(dir.path().join("b")).unwrap();
    std::fs::write(dir.path().join("f"), b"x").unwrap();
    let a = ObjectId::of_path(&dir.path().join("a")).unwrap();
    assert_eq!(a, ObjectId::of_path(&dir.path().join("a")).unwrap());
    assert_ne!(a, ObjectId::of_path(&dir.path().join("b")).unwrap());
    assert_ne!(a, ObjectId::of_path(&dir.path().join("f")).unwrap());
}

#[test]
fn a_folder_swapped_for_another_after_it_was_pinned_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let pinned = ObjectId::of_path(&out).unwrap();
    check_pinned(&out, Some(pinned)).unwrap();
    // Another folder takes the name, while the first is still there under another one.
    std::fs::rename(&out, dir.path().join("moved")).unwrap();
    std::fs::create_dir(&out).unwrap();
    let refused = check_pinned(&out, Some(pinned)).unwrap_err();
    assert!(matches!(refused, Error::Policy(_)), "{refused}");
    // No pin, no check.
    check_pinned(&out, None).unwrap();
}

#[test]
fn a_folder_that_is_gone_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out");
    std::fs::create_dir(&out).unwrap();
    let pinned = ObjectId::of_path(&out).unwrap();
    std::fs::remove_dir(&out).unwrap();
    assert!(check_pinned(&out, Some(pinned)).is_err());
}

#[test]
fn a_file_with_two_names_reports_both_from_its_handle() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("one");
    std::fs::write(&first, b"x").unwrap();
    assert_eq!(facts(&open_file(&first).unwrap()).unwrap().links, 1);
    std::fs::hard_link(&first, dir.path().join("two")).unwrap();
    assert_eq!(facts(&open_file(&first).unwrap()).unwrap().links, 2);
}

#[test]
fn only_a_name_surrogate_tag_is_a_link() {
    // The tags the system defines, by what they stand for.
    let surrogates = [
        0xA000_000C_u32, // IO_REPARSE_TAG_SYMLINK
        0xA000_0003,     // IO_REPARSE_TAG_MOUNT_POINT
    ];
    let files = [
        0x8000_0008_u32, // IO_REPARSE_TAG_WIM
        0x8000_0013,     // IO_REPARSE_TAG_DEDUP
        0x8000_0018,     // IO_REPARSE_TAG_WCI
        0x8000_001B,     // IO_REPARSE_TAG_APPEXECLINK
        0x8000_0023,     // IO_REPARSE_TAG_AF_UNIX
        0x8000_101A,     // OneDrive placeholder
        0x9000_001A,     // IO_REPARSE_TAG_CLOUD_9 placeholder
        0x9000_001C,     // IO_REPARSE_TAG_PROJFS
    ];
    for tag in surrogates {
        assert!(is_name_surrogate_tag(tag), "{tag:#x}");
    }
    for tag in files {
        assert!(!is_name_surrogate_tag(tag), "{tag:#x}");
    }
}

#[test]
fn a_plain_file_and_folder_are_not_links_and_have_distinct_128_bit_ids() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("f"), b"x").unwrap();
    assert!(!is_link(&open_file(&dir.path().join("f")).unwrap()).unwrap());
    assert!(!is_link(&open_folder(dir.path()).unwrap()).unwrap());
    assert_ne!(
        ObjectId::of_path(&dir.path().join("f")).unwrap(),
        ObjectId::of_path(dir.path()).unwrap()
    );
}
