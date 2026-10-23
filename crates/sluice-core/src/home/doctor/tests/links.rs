//! What doctor says about a home given as a link, and about who owns it. Every message names the
//! home as the user gave it, and not the folder the link leads to, and a home is read by the path
//! the system gave for it, as `Home::open` writes it.

use std::fs;
use std::path::Path;

use super::{Finding, Name, finding};
use crate::Home;

/// A link at `link` to the folder `target`: a symlink on Unix, a junction on Windows, which a user
/// may make without the right a Windows symlink needs.
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

/// The text of a finding that carries one.
fn why(finding: &Finding) -> &str {
    match finding {
        Finding::LayoutUnreadable(why)
        | Finding::LockUnreadable(why)
        | Finding::KeyUnreadable(why)
        | Finding::HomeUnreadable(_, why) => why,
        other => panic!("not an unreadable finding: {other:?}"),
    }
}

/// A `VERSION` that is a folder cannot be read, on every platform. The layout check names the home
/// as given, where the read said the folder its link leads to.
#[test]
fn an_unreadable_layout_names_the_path_given() {
    let parent = tempfile::tempdir().unwrap();
    let (target, link) = (parent.path().join("target"), parent.path().join("home"));
    drop(Home::open(&target).unwrap());
    fs::remove_file(target.join("VERSION")).unwrap();
    fs::create_dir(target.join("VERSION")).unwrap();
    link_folder(&target, &link);
    let found = finding(&link, Name::Layout);
    let text = why(&found);
    assert!(
        text.contains(&link.join("VERSION").display().to_string()),
        "{text}"
    );
    assert!(!text.contains(&target.display().to_string()), "{text}");
}

/// A link at `lock` is never followed, so it is an unreadable lock, whatever platform it is on.
#[test]
fn an_unreadable_lock_names_the_path_given() {
    let parent = tempfile::tempdir().unwrap();
    let (target, link) = (parent.path().join("target"), parent.path().join("home"));
    let elsewhere = parent.path().join("elsewhere");
    drop(Home::open(&target).unwrap());
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_file(target.join("lock")).unwrap();
    link_folder(&elsewhere, &target.join("lock"));
    link_folder(&target, &link);
    let found = finding(&link, Name::Lock);
    let text = why(&found);
    assert!(
        text.contains(&link.join("lock").display().to_string()),
        "{text}"
    );
    assert!(!text.contains(&target.display().to_string()), "{text}");
}

/// Nothing of the home can be listed when its folder has no access at all, so the layout, the lock
/// and the key all fail to read, and each names the path given. Root reads it anyway, and then
/// there is nothing to check.
#[cfg(unix)]
#[test]
fn an_unreadable_key_names_the_path_given() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let (target, link) = (parent.path().join("target"), parent.path().join("home"));
    Home::open(&target).unwrap().identity().unwrap();
    link_folder(&target, &link);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&target).is_ok();
    let found = [
        finding(&link, Name::Layout),
        finding(&link, Name::Lock),
        finding(&link, Name::Key),
    ];
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    if readable {
        eprintln!("SKIPPED: this user can read a folder with mode 0000");
        return;
    }
    for (found, file) in found.iter().zip(["VERSION", "lock", "identity.key"]) {
        let text = why(found);
        let named = link.join(file).display().to_string();
        assert!(text.contains(&named), "{found:?}");
        assert!(!text.contains(&target.display().to_string()), "{text}");
    }
}
