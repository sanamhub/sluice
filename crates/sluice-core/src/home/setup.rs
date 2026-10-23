//! The home folder and the three folders in it: made if they are missing, and a new home made
//! private before anything is written in it.
//!
//! The home's path is resolved once, every link followed, and everything after works on the
//! folder the files really land in, by the path the system gave: on Windows with its `\\?\`
//! prefix, which the Win32 calls take literally. Without it they normalise a path again, so a
//! component with a trailing dot or space can name another folder, a component named `CON` can
//! reach a device, and a path over 260 characters fails. A Windows junction or a Unix symlink given
//! as the home would otherwise be made private itself, as Windows does when a list is set on a
//! junction, while the folders and the key went into its target with the access the target
//! inherited.

use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::storage;
use crate::Error;

mod private;

/// The handle-based owner check, for the tests of doctor and of the mode change.
#[cfg(all(test, unix))]
pub(in crate::home) use private::keep_home_private_for;

/// The folders `open` creates under the home, besides the home itself.
pub(super) const FOLDERS: [&str; 3] = ["blobs", "db", "logs"];

/// The files sluice makes in a home, and the temporary names it makes them under. Another process
/// opening the same new home at the same moment may have made any of them before this one looks.
///
/// `identity.key` and `identity.key.tmp` are missing on purpose: the second look never accepts a
/// key, not even one this user made, because a key planted in the gap is a key another user knows.
/// `Home::open` never creates either file, so only a process that already holds the lock and is
/// making an identity can be the one whose key is refused; the operator runs the command again.
const FILES: [&str; 5] = ["VERSION", "VERSION.tmp", "lock", "lock.pid", "lock.pid.tmp"];

/// How long a second look waits, so a process that is half way through making a home has finished
/// its files. Long next to a few file creations, and short next to a command.
const SETTLE: Duration = Duration::from_millis(50);

/// Creates the home and its folders if they are missing, and returns the home with every link
/// followed. On Unix they get mode 0700, so the identity key is not readable through the folder
/// even if its own mode were loosened by hand. On Windows a home that is new gets a private access
/// list before anything is made inside it, so the folders inside inherit it.
///
/// A home folder that is already there and holds nothing at all is new too: a `$SLUICE_HOME` the
/// user made first would otherwise keep the access it inherited, and doctor would fail it forever.
/// A folder that holds anything, a hidden file or an empty folder included, is left as it is, since
/// what it holds may be the user's, and doctor reports its access.
///
/// Errors name the home by the path given, not by where its links lead.
pub(super) fn create_folders(home: &Path) -> Result<PathBuf, Error> {
    create_folders_with(home, private::keep_home_private)
}

/// [`create_folders`] with the step that makes an empty home private as a parameter, so a test can
/// put something in the gap.
pub(super) fn create_folders_with(
    home: &Path,
    restrict: fn(&Path) -> Result<(), Error>,
) -> Result<PathBuf, Error> {
    let created = create_home(home)?;
    let root = resolve(home)?;
    prepare(&root, created, restrict).map_err(|e| super::as_given(e, &root, home))?;
    Ok(root)
}

/// Makes the home private if it is new, and makes its folders. `restrict` is what makes a folder
/// private, a parameter for the same reason as in [`create_folders_with`].
pub(super) fn prepare(
    root: &Path,
    created: bool,
    restrict: fn(&Path) -> Result<(), Error>,
) -> Result<(), Error> {
    prepare_with(root, created, restrict, owned_by_this_user)
}

/// [`prepare`] with the owner check as a parameter, so a test can be an entry another user made.
/// An entry that this user did not make is not accepted however it is named, since its owner can
/// change what it is after the check.
pub(super) fn prepare_with(
    root: &Path,
    created: bool,
    restrict: fn(&Path) -> Result<(), Error>,
    owner: fn(&Path) -> io::Result<bool>,
) -> Result<(), Error> {
    if created || is_empty(root)? {
        private_while_empty(root, restrict, owner)?;
    }
    let builder = folder_builder(false);
    for name in FOLDERS {
        let folder = root.join(name);
        builder
            .create(&folder)
            .or_else(|e| exists_as_folder(e, &folder))
            .map_err(|e| storage("create the folder", &folder, e))?;
        real_folder(&folder)?;
    }
    Ok(())
}

/// `path` with every link followed, as `std::fs::canonicalize` gives it. On Windows that is the
/// verbatim form, `\\?\C:\...`, and it stays that way here: every call that touches the home takes
/// it, and a message shows the path as the user gave it (see `as_given`).
///
/// Some file systems cannot be asked: a RAM disk, a WinFsp or Dokan drive, a VirtualBox share, an
/// old SMB server. When the system answers that the call is unsupported, and the home itself is
/// not a link, the path given is the one to use. Any other error is refused, with a verb that fits
/// it: access denied is not "cannot find".
pub(super) fn resolve(path: &Path) -> Result<PathBuf, Error> {
    resolve_with(path, |path| fs::canonicalize(path))
}

/// [`resolve`] with the call that follows links as a parameter, so a test can be a file system that
/// does not have it.
pub(super) fn resolve_with(
    path: &Path,
    canonicalize: fn(&Path) -> io::Result<PathBuf>,
) -> Result<PathBuf, Error> {
    match canonicalize(path) {
        Ok(root) => Ok(root),
        Err(e) if cannot_ask(&e) => match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => Err(cannot_resolve(path, &e)),
            Ok(_) => absolute_without_links(path),
            Err(other) => Err(cannot_resolve(path, &other)),
        },
        Err(e) => Err(cannot_resolve(path, &e)),
    }
}

/// The path given, made absolute, after every ancestor is checked with `symlink_metadata` and any
/// link is refused by name. A file system that cannot resolve a path cannot say where it leads, so
/// the one thing left is to walk it. The home itself was checked by the caller; a link higher up
/// would still put everything under wherever it points.
fn absolute_without_links(path: &Path) -> Result<PathBuf, Error> {
    let absolute = std::path::absolute(path).map_err(|e| storage("resolve", path, e))?;
    for ancestor in absolute.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Policy(format!(
                    "{} is a link, and this file system cannot say where a path leads, so sluice \
                     cannot check where the home is: remove the link, or give the home's real path",
                    ancestor.display()
                )));
            }
            Ok(_) => {}
            Err(e) => return Err(storage("read", ancestor, e)),
        }
    }
    Ok(absolute)
}

/// The file system does not implement the call that follows links, as opposed to refusing it.
fn cannot_ask(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::Unsupported || invalid_function(error)
}

/// `ERROR_INVALID_FUNCTION`, which a Windows file system answers when it has no such call. Only on
/// Windows, where the number means that: on Unix 1 is `EPERM`.
#[cfg(windows)]
fn invalid_function(error: &io::Error) -> bool {
    use windows_sys::Win32::Foundation::ERROR_INVALID_FUNCTION;
    error.raw_os_error() == Some(ERROR_INVALID_FUNCTION as i32)
}

#[cfg(not(windows))]
fn invalid_function(_error: &io::Error) -> bool {
    false
}

/// The refusal for a home that cannot be resolved, by the path the user gave.
fn cannot_resolve(path: &Path, error: &io::Error) -> Error {
    let verb = match error.kind() {
        io::ErrorKind::NotFound => "find",
        io::ErrorKind::PermissionDenied => "get access to",
        _ => "resolve",
    };
    storage(verb, path, io::Error::new(error.kind(), error.to_string()))
}

/// Makes an empty home private, then lists it again. Anything that appeared in between was made
/// with the access the folder had before, possibly by another user, so the open stops and names
/// the home, unless it is only what sluice itself makes. Another sluice opening the same new home
/// at the same moment is the usual cause: it is given a moment to finish, and listed once more.
/// `restrict` is a parameter so a test can put a file in the gap.
fn private_while_empty(
    home: &Path,
    restrict: fn(&Path) -> Result<(), Error>,
    owner: fn(&Path) -> io::Result<bool>,
) -> Result<(), Error> {
    restrict(home)?;
    if is_empty(home)? {
        return Ok(());
    }
    std::thread::sleep(SETTLE);
    if holds_only_what_sluice_makes(home, owner)? {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} was empty, and something appeared in it while sluice made it private. Look at what \
         is there before using it as the home; if another sluice was opening it at the same time, \
         run the command again",
        home.display()
    )))
}

/// Whether every entry is a file or folder of the names sluice makes, and is what it should be: a
/// real folder for `blobs`, `db` and `logs`, a plain file for the rest, and owned by this user. An
/// entry this user did not make is not accepted: another user who can write in the folder can
/// plant a key, a lock or a `VERSION` there, and their owner can change it again after the check.
/// A link by one of those names is not accepted either.
fn holds_only_what_sluice_makes(
    home: &Path,
    owner: fn(&Path) -> io::Result<bool>,
) -> Result<bool, Error> {
    for entry in fs::read_dir(home).map_err(|e| storage("list", home, e))? {
        let entry = entry.map_err(|e| storage("list", home, e))?;
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            return Ok(false);
        };
        let made = match entry.file_name().to_str() {
            Some(name) if FOLDERS.contains(&name) => {
                meta.is_dir() && !meta.file_type().is_symlink()
            }
            Some(name) if FILES.contains(&name) => meta.is_file(),
            _ => false,
        };
        if !made {
            return Ok(false);
        }
        let ours = owner(&path).map_err(|e| storage("read the owner of", &path, e))?;
        if !ours {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Makes the parents of the home, then the home, and says whether the home was new. The home is
/// made on its own because only a call that creates one folder can tell.
fn create_home(home: &Path) -> Result<bool, Error> {
    if let Some(parent) = home.parent().filter(|p| !p.as_os_str().is_empty()) {
        folder_builder(true)
            .create(parent)
            .map_err(|e| storage("create the folder", parent, e))?;
    }
    match folder_builder(false).create(home) {
        Ok(()) => Ok(true),
        Err(e) => exists_as_folder(e, home)
            .map(|()| false)
            .map_err(|e| storage("create the folder", home, e)),
    }
}

/// Whether the folder holds nothing at all, so no `VERSION` either. A hidden file or an empty
/// folder is something.
fn is_empty(home: &Path) -> Result<bool, Error> {
    let mut entries = fs::read_dir(home).map_err(|e| storage("list", home, e))?;
    Ok(entries.next().is_none())
}

/// An error from creating a folder that is not an error if the folder is already there.
fn exists_as_folder(error: io::Error, path: &Path) -> io::Result<()> {
    if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() {
        Ok(())
    } else {
        Err(error)
    }
}

/// `blobs`, `db` and `logs` are folders of the home itself. A link there would put the store's
/// files wherever it leads, with the access that place has. On Windows `is_symlink` is true for a
/// symlink and for a junction, the two ways to point a folder elsewhere, and for no other reparse
/// point: OneDrive Files On-Demand, deduplication and WCI folders carry other tags, and a home
/// under one of them must not be refused for good.
fn real_folder(folder: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(folder).map_err(|e| storage("read", folder, e))?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} is a link and not a folder of the home: remove it, and sluice makes the folder",
        folder.display()
    )))
}

/// Whether this user made the entry at `path`: on Unix its uid is the effective uid, and on
/// Windows its owner is this user or `Administrators`. The owner is read from a handle opened
/// without following a link, so it is the owner of the object the name holds now.
#[cfg(unix)]
fn owned_by_this_user(path: &Path) -> io::Result<bool> {
    use rustix::fs::{Mode, OFlags};
    let file = rustix::fs::open(path, OFlags::NOFOLLOW | OFlags::CLOEXEC, Mode::empty())?;
    let stat = rustix::fs::fstat(&file)?;
    Ok(stat.st_uid == rustix::process::geteuid().as_raw())
}

#[cfg(windows)]
fn owned_by_this_user(path: &Path) -> io::Result<bool> {
    super::private_windows::is_owned_by_this_user(path)
}

#[cfg(not(any(unix, windows)))]
fn owned_by_this_user(_path: &Path) -> io::Result<bool> {
    Ok(true)
}

fn folder_builder(recursive: bool) -> DirBuilder {
    let mut builder = DirBuilder::new();
    builder.recursive(recursive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
}

#[cfg(test)]
mod tests;
