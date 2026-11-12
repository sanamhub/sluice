use super::*;
use std::fs;
use std::path::{Path, PathBuf};

fn at(root: &Path, path: &str) -> RootPath {
    RootPath {
        root: root.to_str().unwrap().to_string(),
        path: path.to_string(),
    }
}

fn one_root() -> (tempfile::TempDir, PathBuf, Roots) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    let roots = Roots::open(std::slice::from_ref(&root)).unwrap();
    (dir, root, roots)
}

/// Makes `link` lead to the folder `target`: a symbolic link, or on Windows a junction, which needs
/// no privilege.
fn link_to_folder(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(made.status.success(), "mklink failed");
    }
}

#[test]
fn a_daemon_with_no_roots_refuses_every_path_and_says_how_to_allow_one() {
    let roots = Roots::none();
    for usage in [Use::Read, Use::Dir] {
        let refused = roots
            .resolve(Some(&at(Path::new("x"), "")), usage)
            .unwrap_err();
        assert_eq!(refused, ApiError::Refused(NO_ROOTS.to_string()));
    }
    assert!(NO_ROOTS.contains("--root DIR"));
}

#[test]
fn a_path_below_a_root_resolves_to_that_root_joined_with_it() {
    let (_dir, root, roots) = one_root();
    fs::create_dir(root.join("inbox")).unwrap();
    let got = roots.resolve(Some(&at(&root, "inbox")), Use::Dir).unwrap();
    let base = roots.resolve(Some(&at(&root, "")), Use::Dir).unwrap();
    assert!(got.path.ends_with("inbox"), "{:?}", got.path);
    assert!(got.path.starts_with(&base.path), "{:?}", got.path);
}

#[test]
fn a_dot_dot_or_an_absolute_path_is_refused_even_if_it_comes_back_inside() {
    let (_dir, root, roots) = one_root();
    fs::create_dir(root.join("a")).unwrap();
    for bad in ["a/../a", "../root", "/etc", "a/./a", "a//a", "C:/x", r"a\b"] {
        let refused = roots.resolve(Some(&at(&root, bad)), Use::Dir);
        assert!(
            matches!(refused, Err(ApiError::Refused(_))),
            "{bad}: {refused:?}"
        );
    }
}

#[test]
fn a_root_the_daemon_was_not_started_with_is_refused() {
    let (dir, _root, roots) = one_root();
    let refused = roots
        .resolve(Some(&at(dir.path(), "")), Use::Dir)
        .unwrap_err();
    assert!(
        refused.to_string().contains("is not a root of this daemon"),
        "{refused}"
    );
}

#[test]
fn a_link_in_the_path_is_refused_whatever_it_leads_to() {
    let (dir, root, roots) = one_root();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"x").unwrap();
    link_to_folder(&root.join("door"), &outside);
    for usage in [Use::Read, Use::Dir] {
        for path in ["door", "door/secret.txt"] {
            let refused = roots.resolve(Some(&at(&root, path)), usage).unwrap_err();
            assert!(refused.to_string().contains("link"), "{path}: {refused}");
        }
    }
}

#[test]
fn a_path_to_send_that_is_not_there_is_refused_and_one_to_write_is_made() {
    let (_dir, root, roots) = one_root();
    assert!(
        roots
            .resolve(Some(&at(&root, "new/deeper")), Use::Read)
            .is_err()
    );
    assert!(!root.join("new").exists(), "a read makes nothing");
    let made = roots
        .resolve(Some(&at(&root, "new/deeper")), Use::Dir)
        .unwrap();
    assert!(made.path.ends_with(Path::new("new").join("deeper")));
    assert!(root.join("new").join("deeper").is_dir());
}

#[test]
fn a_file_where_a_folder_is_needed_is_refused_and_not_replaced() {
    let (_dir, root, roots) = one_root();
    fs::write(root.join("taken"), b"mine").unwrap();
    for usage in [Use::Dir, Use::FileName, Use::Read] {
        let refused = roots.resolve(Some(&at(&root, "taken/inside")), usage);
        assert!(refused.is_err(), "{refused:?}");
    }
    assert_eq!(fs::read(root.join("taken")).unwrap(), b"mine");
}

#[test]
fn a_receipt_name_leaves_the_file_to_the_engine_and_refuses_a_link_of_that_name() {
    let (dir, root, roots) = one_root();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let made = roots
        .resolve(Some(&at(&root, "sub/receipt.json")), Use::FileName)
        .unwrap();
    assert!(made.path.ends_with(Path::new("sub").join("receipt.json")));
    assert!(
        root.join("sub").is_dir(),
        "the folder above the name is made"
    );
    assert!(!made.path.exists(), "the file is the engine's to create");
    link_to_folder(&root.join("sub").join("linked.json"), &outside);
    let refused = roots
        .resolve(Some(&at(&root, "sub/linked.json")), Use::FileName)
        .expect_err("a link of that name is refused");
    assert!(refused.to_string().contains("link"), "{refused}");
}

/// A folder swapped for a link after the walk is caught by the identity the walk returned, which is
/// what the engine checks where it opens the same path. On Windows the swap cannot happen at all
/// while the walk's handles are held, because no share mode lets a held folder be renamed.
#[test]
fn a_component_swapped_for_a_link_after_the_walk_no_longer_names_the_object_the_walk_opened() {
    let (dir, root, roots) = one_root();
    let outside = dir.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::create_dir(root.join("d")).unwrap();
    fs::write(root.join("d").join("f"), b"x").unwrap();
    let opened = roots.resolve(Some(&at(&root, "d/f")), Use::Read).unwrap();
    assert_eq!(
        sluice_core::engine::ObjectId::of_path(&opened.path).unwrap(),
        opened.pin
    );
    let moved = fs::rename(root.join("d"), root.join("gone"));
    if cfg!(windows) {
        assert!(moved.is_err(), "a held folder cannot be renamed on Windows");
        return;
    }
    moved.unwrap();
    link_to_folder(&root.join("d"), &outside);
    fs::write(outside.join("f"), b"elsewhere").unwrap();
    let now = sluice_core::engine::ObjectId::of_path(&opened.path);
    assert_ne!(
        now.ok(),
        Some(opened.pin),
        "the path now leads somewhere else"
    );
}

#[test]
fn a_root_replaced_under_its_name_refuses_every_request_until_a_restart() {
    let (dir, root, roots) = one_root();
    fs::rename(&root, dir.path().join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    let refused = roots.resolve(Some(&at(&root, "")), Use::Dir).unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("not the folder this daemon opened"),
        "{refused}"
    );
}

#[test]
fn a_root_whose_folder_is_gone_is_refused() {
    let (_dir, root, roots) = one_root();
    fs::remove_dir(&root).unwrap();
    assert!(roots.resolve(Some(&at(&root, "")), Use::Dir).is_err());
}

#[test]
fn a_root_that_is_a_file_or_is_given_twice_is_refused_at_start() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"x").unwrap();
    assert!(Roots::open(&[file]).is_err());
    let twice = vec![dir.path().to_path_buf(), dir.path().to_path_buf()];
    assert!(Roots::open(&twice).is_err());
}

#[test]
fn a_file_with_a_second_name_outside_the_root_is_not_sent() {
    let (dir, root, roots) = one_root();
    let first = root.join("one");
    fs::write(&first, b"x").unwrap();
    let alone = roots.resolve(Some(&at(&root, "one")), Use::Read).unwrap();
    alone.refuse_hard_links().unwrap();
    // The other name is outside every root, so no look at the path under the root can find it.
    fs::hard_link(&first, dir.path().join("two")).unwrap();
    let linked = roots.resolve(Some(&at(&root, "one")), Use::Read).unwrap();
    let refused = linked.refuse_hard_links().unwrap_err();
    assert!(
        refused.to_string().contains("more than one name"),
        "{refused}"
    );
}

#[test]
fn a_root_that_is_the_home_holds_it_or_lies_inside_it_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    fs::create_dir_all(home.join("inner")).unwrap();
    let beside = dir.path().join("beside");
    fs::create_dir(&beside).unwrap();
    for bad in [home.clone(), dir.path().to_path_buf(), home.join("inner")] {
        let roots = Roots::open(std::slice::from_ref(&bad)).unwrap();
        let refused = roots.refuse_home(&home).expect_err("overlap is refused");
        assert!(refused.to_string().contains("home"), "{bad:?}: {refused}");
    }
    Roots::open(&[beside]).unwrap().refuse_home(&home).unwrap();
    Roots::none().refuse_home(&home).unwrap();
}
