//! A home given as a link, what counts as an empty home, and what may get in between making one
//! private and using it. The platform halves are in `unix.rs` and `windows.rs`.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::fs;
use std::path::Path;

use super::{private_while_empty, resolve};
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
    let Err(Error::Policy(message)) =
        private_while_empty(dir.path(), restrict_while_a_file_arrives)
    else {
        panic!("a file that arrived was not refused");
    };
    assert!(
        message.contains(&dir.path().display().to_string()),
        "{message}"
    );
    assert!(message.contains("something appeared in it"), "{message}");
    let empty = tempfile::tempdir().unwrap();
    assert!(private_while_empty(empty.path(), restrict).is_ok());
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
    assert_eq!(
        fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "nothing went through it"
    );
}

#[test]
fn a_resolved_path_is_absolute_and_has_no_verbatim_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let resolved = resolve(dir.path()).unwrap();
    assert!(resolved.is_absolute(), "{}", resolved.display());
    assert!(
        !resolved.display().to_string().starts_with(r"\\?\"),
        "{}",
        resolved.display()
    );
    assert!(resolve(&dir.path().join("missing")).is_err());
}
