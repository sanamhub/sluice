//! The home folder and the three folders in it: made if they are missing, and a new home made
//! private before anything is written in it.
//!
//! The home's path is resolved once, every link followed, and everything after works on the
//! folder the files really land in. A Windows junction or a Unix symlink given as the home would
//! otherwise be made private itself, as Windows does when a list is set on a junction, while the
//! folders and the key went into its target with the access the target inherited.

use std::fs::{self, DirBuilder};
use std::io;
use std::path::{Path, PathBuf};

use super::storage;
use crate::Error;

/// The folders `open` creates under the home, besides the home itself.
pub(super) const FOLDERS: [&str; 3] = ["blobs", "db", "logs"];

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
    let created = create_home(home)?;
    let root = resolve(home).map_err(|e| storage("find", home, e))?;
    prepare(&root, created).map_err(|e| super::as_given(e, &root, home))?;
    Ok(root)
}

fn prepare(root: &Path, created: bool) -> Result<(), Error> {
    if created || is_empty(root)? {
        private_while_empty(root, keep_home_private)?;
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

/// `path` with every link followed, and on Windows without the `\\?\` that
/// `std::fs::canonicalize` puts in front, so the path can be shown and compared as the user would
/// write it.
pub(super) fn resolve(path: &Path) -> io::Result<PathBuf> {
    fs::canonicalize(path).map(plain)
}

#[cfg(windows)]
fn plain(path: PathBuf) -> PathBuf {
    use std::ffi::OsString;
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let start = match components.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(letter) => OsString::from(format!("{}:\\", char::from(letter))),
            Prefix::VerbatimUNC(server, share) => {
                let mut start = OsString::from(r"\\");
                start.push(server);
                start.push(r"\");
                start.push(share);
                start.push(r"\");
                start
            }
            _ => return path,
        },
        _ => return path,
    };
    let mut plain = PathBuf::from(start);
    plain.extend(components.filter(|c| !matches!(c, Component::RootDir)));
    plain
}

#[cfg(not(windows))]
fn plain(path: PathBuf) -> PathBuf {
    path
}

/// Makes an empty home private, then lists it again. Anything that appeared in between was made
/// with the access the folder had before, possibly by another user, and is not the store's to
/// keep, so the open stops and names the home. `restrict` is a parameter so a test can put a file
/// in the gap.
fn private_while_empty(home: &Path, restrict: fn(&Path) -> Result<(), Error>) -> Result<(), Error> {
    restrict(home)?;
    if is_empty(home)? {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} was empty, and something appeared in it while sluice made it private. Look at what \
         is there before using it as the home; if another sluice was opening it at the same time, \
         run the command again",
        home.display()
    )))
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

/// `blobs`, `db` and `logs` are folders of the home itself. A link there, or on Windows any
/// reparse point, would put the store's files wherever it leads, with the access that place has.
fn real_folder(folder: &Path) -> Result<(), Error> {
    let meta = fs::symlink_metadata(folder).map_err(|e| storage("read", folder, e))?;
    if meta.is_dir() && !is_link(&meta) {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "{} is a link and not a folder of the home: remove it, and sluice makes the folder",
        folder.display()
    )))
}

#[cfg(windows)]
fn is_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_link(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
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

#[cfg(windows)]
fn keep_home_private(home: &Path) -> Result<(), Error> {
    super::private_windows::restrict_dir(home).map_err(|e| storage("restrict access to", home, e))
}

/// Mode 0700. A folder this call made has it already, and one the user made first may not. A file
/// system that keeps no Unix permissions, such as vfat, refuses the change, and the home is then
/// refused rather than left open to every user.
#[cfg(unix)]
fn keep_home_private(home: &Path) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(home, fs::Permissions::from_mode(0o700)).map_err(|e| {
        Error::Storage(format!(
            "cannot restrict access to {}: {e}. The home must be a folder this user owns, on a \
             file system that keeps Unix permissions, which vfat and exFAT do not",
            home.display()
        ))
    })
}

#[cfg(not(any(unix, windows)))]
fn keep_home_private(_home: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
mod tests;
