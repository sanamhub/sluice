use std::ffi::OsString;
use std::path::PathBuf;

use super::locate_from;
use crate::Error;

fn os(text: &str) -> Option<OsString> {
    Some(OsString::from(text))
}

#[test]
fn sluice_home_wins_when_it_is_set_and_not_empty() {
    let data = Some(PathBuf::from("/data"));
    assert_eq!(
        locate_from(os("/elsewhere"), data.clone()).unwrap(),
        PathBuf::from("/elsewhere")
    );
    // Relative or odd, it is the user's word for it.
    assert_eq!(
        locate_from(os("rel/home"), data).unwrap(),
        PathBuf::from("rel/home")
    );
}

#[test]
fn an_empty_or_missing_sluice_home_falls_back_to_the_data_folder() {
    let data = Some(PathBuf::from("/data"));
    for sluice_home in [None, os("")] {
        let found = locate_from(sluice_home, data.clone()).unwrap();
        assert_eq!(found, PathBuf::from("/data").join("sluice"));
    }
}

#[test]
fn no_data_folder_and_no_sluice_home_is_a_storage_error_that_says_what_to_do() {
    let Err(Error::Storage(message)) = locate_from(None, None) else {
        panic!("expected a storage error");
    };
    assert!(message.contains("SLUICE_HOME"), "{message}");
}

mod resolve {
    use std::cell::RefCell;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    use super::super::setup::resolve_with;
    use crate::Error;

    /// A link at `link` to the folder `target`: a symlink on Unix, a junction on Windows, which a
    /// user may make without the right a Windows symlink needs.
    fn link_folder(target: &Path, link: &Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).unwrap();
        #[cfg(windows)]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .stdout(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(status.success(), "mklink /J failed");
        }
    }

    /// A file system that cannot resolve a path whose last component is `home`, as one that cannot
    /// answer for the folder the home is in, and resolves every other path.
    fn fails_at_home(path: &Path) -> io::Result<PathBuf> {
        if path.file_name().is_some_and(|name| name == "home") {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        fs::canonicalize(path)
    }

    thread_local! {
        static BASE: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// A file system that resolves `BASE` and its ancestors and nothing below it.
    fn fails_below_base(path: &Path) -> io::Result<PathBuf> {
        let base = BASE.with(|cell| cell.borrow().clone()).expect("a base");
        if path == base || base.starts_with(path) {
            fs::canonicalize(path)
        } else {
            Err(io::Error::from(io::ErrorKind::Unsupported))
        }
    }

    /// A link above the ancestor the file system can still resolve was followed by the call that
    /// resolved it, so the resolved path is used and the home opens.
    #[test]
    fn a_link_above_the_ancestor_the_file_system_cannot_resolve_is_accepted() {
        let parent = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(parent.path()).unwrap();
        let real = base.join("real");
        fs::create_dir(&real).unwrap();
        fs::create_dir(real.join("home")).unwrap();
        let link = base.join("link");
        link_folder(&real, &link);
        let resolved = resolve_with(&link.join("home"), fails_at_home).unwrap();
        assert_eq!(resolved, fs::canonicalize(real.join("home")).unwrap());
    }

    /// A link below the ancestor the file system can resolve is refused by name: that file system
    /// cannot say where it leads either.
    #[test]
    fn a_link_below_the_ancestor_the_file_system_cannot_resolve_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(parent.path()).unwrap();
        let real = base.join("real");
        fs::create_dir(&real).unwrap();
        fs::create_dir(real.join("home")).unwrap();
        let link = base.join("link");
        link_folder(&real, &link);
        BASE.with(|cell| *cell.borrow_mut() = Some(base.clone()));
        let Err(Error::Policy(message)) = resolve_with(&link.join("home"), fails_below_base) else {
            panic!("a link below the resolved ancestor was accepted");
        };
        assert!(
            message.contains(&link.display().to_string()),
            "the link is named: {message}"
        );
        assert!(message.contains("is a link"), "{message}");
    }

    /// Every component below the ancestor the file system can resolve is part of the home, two
    /// levels down and three. A tail dropped here would make sluice create its folders in the
    /// ancestor instead of in the home.
    #[test]
    fn every_component_below_the_resolved_ancestor_is_kept() {
        let parent = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(parent.path()).unwrap();
        BASE.with(|cell| *cell.borrow_mut() = Some(base.clone()));
        let two = base.join("a").join("b");
        fs::create_dir_all(&two).unwrap();
        assert_eq!(resolve_with(&two, fails_below_base).unwrap(), two);
        let three = two.join("c");
        fs::create_dir(&three).unwrap();
        assert_eq!(resolve_with(&three, fails_below_base).unwrap(), three);
    }

    /// A link two levels below the resolved ancestor is refused by the path the user gave. A walk
    /// that stopped at the level above it would take the link for a folder and follow it.
    #[test]
    fn a_link_two_levels_below_the_resolved_ancestor_is_refused() {
        let parent = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(parent.path()).unwrap();
        let real = base.join("a").join("real");
        fs::create_dir_all(real.join("home")).unwrap();
        let link = base.join("a").join("link");
        link_folder(&real, &link);
        BASE.with(|cell| *cell.borrow_mut() = Some(base.clone()));
        let Err(Error::Policy(message)) = resolve_with(&link.join("home"), fails_below_base) else {
            panic!("a link two levels below the resolved ancestor was accepted");
        };
        assert!(
            message.contains(&link.display().to_string()),
            "the link is named: {message}"
        );
        assert!(message.contains("is a link"), "{message}");
    }
}

mod as_given {
    use std::path::Path;

    use super::super::{as_given, show};
    use crate::Error;

    #[test]
    fn the_resolved_path_is_written_as_the_caller_gave_it() {
        let text = "cannot open /srv/real/lock: denied, and /srv/real/blobs too";
        let shown = show(text, Path::new("/srv/real"), Path::new("/home/me/link"));
        assert_eq!(
            shown,
            "cannot open /home/me/link/lock: denied, and /home/me/link/blobs too"
        );
        // Nothing to change when they are one path.
        assert_eq!(
            show(text, Path::new("/srv/real"), Path::new("/srv/real")),
            text
        );
    }

    /// A caller who ends the path with a separator would otherwise get `home//blobs`.
    #[test]
    fn a_separator_at_the_end_of_the_given_path_is_not_doubled() {
        let text = "cannot open /srv/real/lock";
        for given in ["/home/me/link/", "/home/me/link//"] {
            assert_eq!(
                show(text, Path::new("/srv/real"), Path::new(given)),
                "cannot open /home/me/link/lock",
                "{given}"
            );
        }
        // A path that is nothing but a separator keeps it.
        assert_eq!(show("at /a/b", Path::new("/a/b"), Path::new("/")), "at /");
    }

    /// `/h/../h` holds the resolved `/h`, and is put in once: a second pass over the result would
    /// find `/h` inside it again.
    #[test]
    fn a_given_path_that_holds_the_resolved_one_is_not_replaced_again() {
        let shown = show("cannot open /h/lock", Path::new("/h"), Path::new("/h/../h"));
        assert_eq!(shown, "cannot open /h/../h/lock");
        let once = Error::Storage("cannot open /h/lock".to_string());
        let Error::Storage(message) = as_given(once, Path::new("/h"), Path::new("/h/../h")) else {
            panic!("the kind changed");
        };
        assert_eq!(message, "cannot open /h/../h/lock");
    }

    /// Only a whole path is replaced: `/h` is not a part of `/hx` or of `/other/h`.
    #[test]
    fn another_path_that_starts_or_ends_alike_is_left_alone() {
        let text = "/hx/lock /other/h/lock /h/lock /h";
        let shown = show(text, Path::new("/h"), Path::new("/g"));
        assert_eq!(shown, "/hx/lock /other/h/lock /g/lock /g");
    }

    /// A separator or the end of the text is the only boundary, so a name that goes on with a dot
    /// or a letter is another folder and is not replaced.
    #[test]
    fn a_path_that_goes_on_with_a_name_is_not_the_resolved_one() {
        let text = "/srv/real.old/lock /srv/realx/lock /srv/real/lock";
        let shown = show(text, Path::new("/srv/real"), Path::new("/g"));
        assert_eq!(shown, "/srv/real.old/lock /srv/realx/lock /g/lock");
    }

    #[test]
    fn only_a_storage_or_policy_message_is_rewritten() {
        let shown = as_given(Error::Cancelled, Path::new("/h"), Path::new("/g"));
        assert!(matches!(shown, Error::Cancelled));
        let policy = Error::Policy("busy: /h".to_string());
        let Error::Policy(message) = as_given(policy, Path::new("/h"), Path::new("/g")) else {
            panic!("the kind changed");
        };
        assert_eq!(message, "busy: /g");
    }
}
