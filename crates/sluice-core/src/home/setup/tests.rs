//! A home given as a link, what counts as an empty home, and what may get in between making one
//! private and using it. The platform halves are in `unix.rs` and `windows.rs`.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::{cannot_resolve, prepare, resolve, resolve_with};
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
    drop(home);
    assert!(is_private(&target), "the target is the home");
    for name in ["blobs", "db", "logs", "identity.key", "VERSION"] {
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
        !dir.path().join("blobs").exists(),
        "the open stopped before it made a folder"
    );
    let empty = tempfile::tempdir().unwrap();
    assert!(prepare(empty.path(), true, restrict).is_ok());
}

/// What a second sluice finds when it opens the same new home a moment after the first: the first
/// one's files, which are exactly what sluice makes, and are accepted after a second look.
#[test]
fn what_sluice_makes_appearing_while_the_home_is_made_private_is_accepted() {
    fn restrict_while_another_sluice_arrives(home: &Path) -> Result<(), Error> {
        fs::create_dir(home.join("blobs")).unwrap();
        fs::write(home.join("VERSION"), "1\n").unwrap();
        fs::write(home.join("lock"), "").unwrap();
        Ok(())
    }
    fn restrict_while_a_link_arrives(home: &Path) -> Result<(), Error> {
        fs::create_dir(home.join("elsewhere")).unwrap();
        link_folder(&home.join("elsewhere"), &home.join("blobs"));
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

#[test]
fn a_link_in_place_of_blobs_is_refused_and_named() {
    let parent = tempfile::tempdir().unwrap();
    let path = parent.path().join("home");
    let elsewhere = parent.path().join("elsewhere");
    fs::create_dir(&path).unwrap();
    fs::create_dir(&elsewhere).unwrap();
    link_folder(&elsewhere, &path.join("blobs"));
    let Err(Error::Policy(message)) = Home::open(&path) else {
        panic!("a link as blobs was not refused");
    };
    assert!(message.contains("blobs"), "{message}");
    assert!(message.contains("is a link"), "{message}");
}

/// A home that is itself a link, with a link as its `blobs`: the message names the home as it was
/// given, and not the folder the link leads to.
#[test]
fn a_linked_home_whose_blobs_is_a_link_is_named_by_the_path_given() {
    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let given = parent.path().join("home");
    let elsewhere = parent.path().join("elsewhere");
    for folder in [&target, &elsewhere] {
        fs::create_dir(folder).unwrap();
    }
    link_folder(&target, &given);
    link_folder(&elsewhere, &target.join("blobs"));
    let Err(Error::Policy(message)) = Home::open(&given) else {
        panic!("a link as blobs was not refused");
    };
    assert!(
        message.contains(&given.join("blobs").display().to_string()),
        "{message}"
    );
    assert!(
        !message.contains(&target.display().to_string()),
        "the target is not named: {message}"
    );
}

#[test]
fn a_resolved_path_is_absolute_and_is_what_the_system_gave() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = resolve(dir.path()).unwrap();
    assert!(resolved.is_absolute(), "{}", resolved.display());
    // On Windows that is the verbatim form, which every call that touches the home takes.
    assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());
    #[cfg(windows)]
    assert!(resolved.display().to_string().starts_with(r"\\?\"));
}

/// The verb says what went wrong: a path that is not there is not found, and one that is refused
/// is not "not found".
#[test]
fn an_error_that_cannot_resolve_has_a_verb_that_fits_it() {
    let dir = tempfile::tempdir().unwrap();
    let Err(Error::Storage(message)) = resolve(&dir.path().join("missing")) else {
        panic!("a missing home was resolved");
    };
    assert!(message.starts_with("cannot find "), "{message}");
    let path = Path::new("/some/home");
    for (kind, verb) in [
        (io::ErrorKind::NotFound, "find"),
        (io::ErrorKind::PermissionDenied, "get access to"),
        (io::ErrorKind::InvalidData, "resolve"),
    ] {
        let Error::Storage(message) = cannot_resolve(path, &io::Error::from(kind)) else {
            panic!("not a storage error");
        };
        let start = format!("cannot {verb} {}", path.display());
        assert!(message.starts_with(&start), "{message}");
    }
}

fn unsupported(_: &Path) -> io::Result<PathBuf> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

fn denied(_: &Path) -> io::Result<PathBuf> {
    Err(io::Error::from(io::ErrorKind::PermissionDenied))
}

/// A RAM disk, a WinFsp or Dokan drive, a VirtualBox share or an old SMB server cannot be asked
/// where a path leads. A home on one that is not a link is the path given.
#[test]
fn a_file_system_that_cannot_be_asked_gives_the_path_when_the_home_is_not_a_link() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(resolve_with(dir.path(), unsupported).unwrap(), dir.path());
    // A link on such a file system is refused: nothing says where it leads.
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    fs::create_dir(&target).unwrap();
    link_folder(&target, &link);
    assert!(resolve_with(&link, unsupported).is_err());
    // A missing home is not found, and a refusal is not "unsupported".
    assert!(resolve_with(&dir.path().join("missing"), unsupported).is_err());
    let Err(Error::Storage(message)) = resolve_with(dir.path(), denied) else {
        panic!("a refusal was resolved");
    };
    assert!(message.starts_with("cannot get access to "), "{message}");
}

#[cfg(windows)]
#[test]
fn error_invalid_function_is_the_same_answer_as_unsupported() {
    fn invalid_function(_: &Path) -> io::Result<PathBuf> {
        Err(io::Error::from_raw_os_error(1))
    }
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        resolve_with(dir.path(), invalid_function).unwrap(),
        dir.path()
    );
}
