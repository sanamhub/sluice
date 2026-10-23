//! What doctor says about a home given as a link, and about who owns it. Every message names the
//! home as the user gave it, and not the folder the link leads to, and a home is read by the path
//! the system gave for it, as `Home::open` writes it.

use std::fs;
use std::path::Path;

use super::{Finding, Name, finding};
use crate::Home;
#[cfg(unix)]
use crate::doctor::{Status, examine_at};

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

/// Nothing of the home can be listed when its folder has no access at all. The home check now sees
/// that too, and names the path the user gave, not the folder the link leads to. Root reads it
/// anyway, and then there is nothing to check.
#[cfg(unix)]
#[test]
fn an_unreadable_home_names_the_path_given() {
    use std::os::unix::fs::PermissionsExt;
    let parent = tempfile::tempdir().unwrap();
    let (target, link) = (parent.path().join("target"), parent.path().join("home"));
    Home::open(&target).unwrap().identity().unwrap();
    link_folder(&target, &link);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o000)).unwrap();
    let readable = fs::read_dir(&target).is_ok();
    let checks = examine_at(&link, true);
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    if readable {
        eprintln!("SKIPPED: this user can read a folder with mode 0000");
        return;
    }
    let Finding::HomeUnreadable(at, _) = checks[0].finding() else {
        panic!("{:?}", checks[0].finding());
    };
    assert_eq!(at.path, link, "the path given, not the target");
    assert!(
        checks[1..]
            .iter()
            .all(|check| check.status() == Status::Skipped),
        "nothing else is checked through a home that cannot be read"
    );
}

/// On Unix, a folder that another user owns is not private whatever its mode says: its owner can
/// change the mode. The user is a parameter, because a folder of another user cannot be made
/// without being root.
#[cfg(unix)]
#[test]
fn a_home_that_another_user_owns_is_not_private() {
    use super::super::checks::home_not_private_for;
    use std::os::unix::fs::MetadataExt;
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    drop(Home::open(&path).unwrap());
    let me = fs::metadata(&path).unwrap().uid();
    assert_eq!(home_not_private_for(&path, &path, me), None);
    assert_eq!(
        home_not_private_for(&path, &path, me.wrapping_add(1)),
        Some(Finding::AccessOwnedByOther)
    );
    // The owner comes before the mode: a folder open to everyone, owned by someone else, is first
    // of all someone else's.
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        home_not_private_for(&path, &path, me.wrapping_add(1)),
        Some(Finding::AccessOwnedByOther)
    );
    assert_eq!(
        home_not_private_for(&path, &path, me),
        Some(Finding::AccessOpenToOthers(0o755))
    );
}

/// The same rule on the other side: a fresh home whose owner is another user is refused by the
/// open, with a policy error, and doctor flags the same owner. The owner is injected, because a
/// folder of another user cannot be made without being root.
#[cfg(unix)]
#[test]
fn a_fresh_home_another_user_owns_is_refused_by_open_and_named_by_doctor() {
    use super::super::checks::home_not_private_for;
    use crate::home::setup::keep_home_private_for;
    let me = rustix::process::geteuid().as_raw();
    let other = me.wrapping_add(1);
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    fs::create_dir(&path).unwrap();
    assert_eq!(
        home_not_private_for(&path, &path, other),
        Some(Finding::AccessOwnedByOther)
    );
    let Err(crate::Error::Policy(message)) = keep_home_private_for(&path, other) else {
        panic!("a home owned by another user was accepted");
    };
    assert!(message.contains("belongs to another user"), "{message}");
    assert!(!message.contains(&other.to_string()), "no uid: {message}");
    assert!(
        fs::read_dir(&path).unwrap().next().is_none(),
        "nothing was written"
    );
}

/// A link at one of the home's folders is what `open` refuses and what doctor used to follow with
/// `is_dir()`. Both fail it now, and the report names the link.
#[test]
fn a_blobs_that_is_a_link_fails_doctor_and_open() {
    use crate::Error;
    use crate::ErrorKind;
    use crate::doctor::Check;
    let parent = tempfile::tempdir().unwrap();
    let home = parent.path().join("home");
    let elsewhere = parent.path().join("elsewhere");
    drop(Home::open(&home).unwrap());
    fs::create_dir(&elsewhere).unwrap();
    fs::remove_dir(home.join("blobs")).unwrap();
    link_folder(&elsewhere, &home.join("blobs"));
    let found = finding(&home, Name::Home);
    let Finding::HomeFolderIsLink(at, folder) = found.clone() else {
        panic!("{found:?}");
    };
    assert_eq!(folder, "blobs");
    assert_eq!(at.path, home);
    assert_eq!(
        Check::new(found).failure(),
        Some(ErrorKind::Policy),
        "a link is a policy failure"
    );
    let Err(Error::Policy(message)) = Home::open(&home) else {
        panic!("a linked blobs was opened");
    };
    assert!(message.contains("blobs"), "{message}");
    assert!(message.contains("is a link"), "{message}");
}
