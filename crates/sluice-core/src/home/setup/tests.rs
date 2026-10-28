//! A home given as a link, what counts as an empty home, and what may get in between making one
//! private and using it. The platform halves are in `unix.rs` and `windows.rs`.

#[cfg(unix)]
mod mode;
mod owners;
mod resolve;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{prepare, prepare_with};
use crate::doctor::{Finding, Name, Status, examine_at};
use crate::{Error, Home};

/// Whether the folder has what a new home gets: mode 0700 on Unix, the private list on Windows.
fn is_private(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777 == 0o700
    }
    #[cfg(windows)]
    {
        super::super::private_windows::is_private_dir(path).unwrap()
    }
}

/// A folder anyone may read, so that `is_private` is false until sluice changes it.
fn open_folder(path: &Path) {
    fs::create_dir(path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert!(!is_private(path), "{} starts private", path.display());
}

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

fn failed(home: &Path) -> Vec<Name> {
    examine_at(home, true)
        .iter()
        .filter(|check| check.status() == Status::Failed)
        .map(|check| check.name())
        .collect()
}

#[test]
fn a_link_to_an_empty_folder_makes_the_target_private_and_doctor_passes_it() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let link = parent.path().join("home");
    open_folder(&target);
    link_folder(&target, &link);
    let home = Home::open(&link).unwrap();
    home.identity().unwrap();
    assert_eq!(home.path(), link, "the path as given");
    // The one thing a Unix symlink does not show by itself: chmod, mkdir and open all follow it,
    // so the target is private and holds the files with or without the resolve. What the home
    // holds as the place of its files is the resolved path, which is not the link.
    assert_eq!(home.root, fs::canonicalize(&target).unwrap());
    assert_ne!(home.root, link);
    drop(home);
    assert!(is_private(&target), "the target is the home");
    for name in ["db", "db", "logs", "identity.key", "VERSION"] {
        assert!(target.join(name).exists(), "{name} in the target");
    }
    assert_eq!(failed(&link), Vec::<Name>::new());
    let access = examine_at(&link, true)
        .into_iter()
        .find(|check| check.name() == Name::Access)
        .unwrap();
    assert_eq!(access.finding(), &Finding::AccessPrivate);
}

#[test]
fn a_folder_that_holds_only_an_empty_folder_is_not_new_and_is_left_alone() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    open_folder(&path);
    fs::create_dir(path.join("empty")).unwrap();
    drop(Home::open(&path).unwrap());
    assert!(
        !is_private(&path),
        "a folder that holds a folder keeps its access"
    );
    assert!(failed(&path).contains(&Name::Access));
}

#[test]
fn something_that_appears_while_the_home_is_made_private_stops_the_open() {
    // Stands for the change of access, and for another user's file arriving while it runs.
    fn restrict_while_a_file_arrives(home: &Path) -> Result<(), Error> {
        fs::write(home.join("arrived"), "not ours").unwrap();
        Ok(())
    }
    fn restrict(_home: &Path) -> Result<(), Error> {
        Ok(())
    }
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Policy(message)) = prepare(dir.path(), true, restrict_while_a_file_arrives)
    else {
        panic!("a file that arrived was not refused");
    };
    assert!(
        message.contains(&dir.path().display().to_string()),
        "{message}"
    );
    assert!(message.contains("something appeared in it"), "{message}");
    assert!(
        !dir.path().join("db").exists(),
        "the open stopped before it made a folder"
    );
    let empty = tempfile::tempdir().unwrap();
    assert!(prepare(empty.path(), true, restrict).is_ok());
}

/// A key is never what sluice makes: `Home::open` does not create one, so a key that appears in the
/// gap between making the home private and listing it was put there by someone else, even when the
/// owner check would accept it as this user's. It is refused.
#[test]
fn a_key_planted_in_the_gap_is_refused() {
    fn restrict_while_a_key_arrives(home: &Path) -> Result<(), Error> {
        fs::write(home.join("identity.key"), [7u8; 32]).unwrap();
        Ok(())
    }
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Policy(message)) = prepare(dir.path(), true, restrict_while_a_key_arrives)
    else {
        panic!("a planted identity.key was accepted");
    };
    assert!(message.contains("something appeared in it"), "{message}");
}

/// A name sluice makes is not enough: the entry has to be this user's, because its owner can
/// change what it is after the second look and before the key is read. The owner check is the
/// parameter, so the test can be another user on every platform.
#[test]
fn an_entry_this_user_did_not_make_is_refused() {
    fn restrict_while_a_version_arrives(home: &Path) -> Result<(), Error> {
        fs::write(home.join("VERSION"), "1\n").unwrap();
        Ok(())
    }
    fn not_ours(_: &Path) -> io::Result<bool> {
        Ok(false)
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        prepare_with(dir.path(), true, restrict_while_a_version_arrives, not_ours),
        Err(Error::Policy(_))
    ));
    // The same entry, owned by this user, is what a second sluice makes and is accepted.
    let dir = tempfile::tempdir().unwrap();
    prepare_with(dir.path(), true, restrict_while_a_version_arrives, |_| {
        Ok(true)
    })
    .unwrap();
}

/// What a second sluice finds when it opens the same new home a moment after the first: the first
/// one's files, which are exactly what sluice makes, and are accepted after a second look.
#[test]
fn what_sluice_makes_appearing_while_the_home_is_made_private_is_accepted() {
    fn restrict_while_another_sluice_arrives(home: &Path) -> Result<(), Error> {
        fs::create_dir(home.join("db")).unwrap();
        fs::write(home.join("VERSION"), "1\n").unwrap();
        fs::write(home.join("lock"), "").unwrap();
        Ok(())
    }
    fn restrict_while_a_link_arrives(home: &Path) -> Result<(), Error> {
        fs::create_dir(home.join("elsewhere")).unwrap();
        link_folder(&home.join("elsewhere"), &home.join("db"));
        Ok(())
    }
    let dir = tempfile::tempdir().unwrap();
    prepare(dir.path(), true, restrict_while_another_sluice_arrives).unwrap();
    // The same name as a folder of the home is not enough: a link there is not what sluice makes.
    let dir = tempfile::tempdir().unwrap();
    assert!(matches!(
        prepare(dir.path(), true, restrict_while_a_link_arrives),
        Err(Error::Policy(_))
    ));
}

/// Another sluice renames its temporary files while this one lists the home. A name that was in
/// the listing and is gone when it is looked at is not evidence of something else: the listing is
/// read again, and the files that are there are the ones sluice makes.
#[test]
fn a_temporary_name_that_is_renamed_while_the_home_is_listed_is_accepted() {
    fn restrict_while_two_temporaries_arrive(home: &Path) -> Result<(), Error> {
        fs::create_dir(home.join("db")).unwrap();
        fs::write(home.join("VERSION.tmp"), "1\n").unwrap();
        fs::write(home.join("lock.pid.tmp"), "{}\n").unwrap();
        Ok(())
    }
    fn rename_them_on_the_first_look(home: &Path) -> io::Result<bool> {
        let parent = home.parent().expect("a home below the temporary folder");
        let lock = parent.join("lock.pid.tmp");
        if lock.exists() {
            fs::rename(&lock, parent.join("lock.pid")).unwrap();
            fs::rename(parent.join("VERSION.tmp"), parent.join("VERSION")).unwrap();
        }
        Ok(true)
    }
    let dir = tempfile::tempdir().unwrap();
    prepare_with(
        dir.path(),
        true,
        restrict_while_two_temporaries_arrive,
        rename_them_on_the_first_look,
    )
    .unwrap();
}

/// Two opens of one new home at once, which is two `sluice` commands started together. Each gets
/// through making it private, and neither is told that something appeared in it. The lock then goes
/// to one of them, and the other is refused for that, which is a different and expected answer.
#[test]
fn two_opens_of_one_new_home_at_once_both_get_through_50_times() {
    let parent = tempfile::tempdir().unwrap();
    for round in 0..50 {
        let path = parent.path().join(format!("home{round}"));
        let start = std::sync::Barrier::new(2);
        let results: Vec<Result<PathBuf, Error>> = std::thread::scope(|scope| {
            let open = || {
                start.wait();
                super::create_folders(&path)
            };
            let (first, second) = (scope.spawn(open), scope.spawn(open));
            vec![first.join().unwrap(), second.join().unwrap()]
        });
        for result in results {
            assert!(result.is_ok(), "round {round}: {result:?}");
        }
        assert!(is_private(&path), "round {round}");
    }
}

/// The same race through `Home::open`: whichever gets the lock holds the home, and the other says
/// so. Nothing else is an acceptable answer.
#[test]
fn two_opens_through_home_open_are_either_open_or_refused_for_the_lock() {
    let parent = tempfile::tempdir().unwrap();
    for round in 0..50 {
        let path = parent.path().join(format!("home{round}"));
        let start = std::sync::Barrier::new(2);
        let results: Vec<Result<Home, Error>> = std::thread::scope(|scope| {
            let open = || {
                start.wait();
                Home::open(&path)
            };
            let (first, second) = (scope.spawn(open), scope.spawn(open));
            vec![first.join().unwrap(), second.join().unwrap()]
        });
        assert!(
            results.iter().any(Result::is_ok),
            "round {round}: {results:?}"
        );
        for result in results {
            if let Err(error) = result {
                let Error::Policy(message) = error else {
                    panic!("round {round}: {error:?}");
                };
                assert!(message.contains("is using the store"), "{message}");
            }
        }
    }
}
