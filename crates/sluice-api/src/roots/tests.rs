use super::*;

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
    for usage in [Use::Read, Use::Write] {
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
    let got = roots
        .resolve(Some(&at(&root, "inbox")), Use::Write)
        .unwrap();
    assert_eq!(
        got,
        roots
            .resolve(Some(&at(&root, "")), Use::Write)
            .unwrap()
            .join("inbox")
    );
}

#[test]
fn a_dot_dot_or_an_absolute_path_is_refused_even_if_it_comes_back_inside() {
    let (_dir, root, roots) = one_root();
    fs::create_dir(root.join("a")).unwrap();
    for bad in ["a/../a", "../root", "/etc", "a/./a", "a//a", "C:/x", r"a\b"] {
        let refused = roots.resolve(Some(&at(&root, bad)), Use::Write);
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
        .resolve(Some(&at(dir.path(), "")), Use::Write)
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
    for usage in [Use::Read, Use::Write] {
        for path in ["door", "door/secret.txt"] {
            let refused = roots.resolve(Some(&at(&root, path)), usage).unwrap_err();
            assert!(refused.to_string().contains("link"), "{path}: {refused}");
        }
    }
}

#[test]
fn a_path_to_send_that_is_not_there_is_refused_and_one_to_write_is_not() {
    let (_dir, root, roots) = one_root();
    assert!(
        roots
            .resolve(Some(&at(&root, "new/deeper")), Use::Read)
            .is_err()
    );
    let made = roots
        .resolve(Some(&at(&root, "new/deeper")), Use::Write)
        .unwrap();
    assert!(made.ends_with(Path::new("new").join("deeper")));
}

#[test]
fn a_root_replaced_under_its_name_refuses_every_request_until_a_restart() {
    let (dir, root, roots) = one_root();
    fs::rename(&root, dir.path().join("moved")).unwrap();
    fs::create_dir(&root).unwrap();
    let refused = roots.resolve(Some(&at(&root, "")), Use::Write).unwrap_err();
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
    assert!(roots.resolve(Some(&at(&root, "")), Use::Write).is_err());
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
fn a_file_with_two_names_is_not_sent() {
    let (dir, root, _roots) = one_root();
    let first = root.join("one");
    fs::write(&first, b"x").unwrap();
    refuse_hard_links(&first).unwrap();
    fs::hard_link(&first, dir.path().join("two")).unwrap();
    let refused = refuse_hard_links(&first).unwrap_err();
    assert!(
        refused.to_string().contains("more than one name"),
        "{refused}"
    );
}
