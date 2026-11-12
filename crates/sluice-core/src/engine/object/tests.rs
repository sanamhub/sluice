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
