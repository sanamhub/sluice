//! The folder checks of the home layout: whether a folder is a real folder of this user, and the
//! identity of the one that was checked. The kind, the owner and the identity come from one open
//! handle, so a name swapped for another object between two of them cannot mix the answers.

use std::path::Path;

use crate::Error;

/// Refuses `folder` unless it is a real folder of this user: not a link, and not another user's.
/// Read before anything is read out of a folder that holds a store, because a link there would lead
/// somewhere else and another user's folder can be changed after anything sluice writes in it.
pub(crate) fn check_folder(folder: &Path) -> Result<(), Error> {
    check_folder_id(folder).map(|_| ())
}

/// [`check_folder`] with the identity of the folder it checked. The kind, the owner and the
/// identity come from one open handle, so a name swapped for another object between two of them
/// cannot mix the answers: a caller that opens a store in the folder later compares this identity
/// with the folder the store was opened in, and one folder swapped for another is noticed even when
/// both carry the same marker.
pub(crate) fn check_folder_id(folder: &Path) -> Result<Option<(u64, u64)>, Error> {
    imp::check(folder)
}

/// The refusal for a name that is a link or is not a folder.
fn not_a_folder(folder: &Path) -> Error {
    Error::Policy(format!(
        "{} is a link or is not a folder, so it cannot hold a partial store; remove it",
        folder.display()
    ))
}

#[cfg(unix)]
mod imp {
    use std::{fs, path::Path};

    use rustix::fs::{FileType, Mode, OFlags};
    use rustix::io::Errno;

    use super::not_a_folder;
    use crate::Error;
    use crate::home::setup::owner::{accepted_uid, owned_by_this_user};
    use crate::home::setup::private;
    use crate::home::storage;

    pub(super) fn check(folder: &Path) -> Result<Option<(u64, u64)>, Error> {
        // `NOFOLLOW` makes a link at the name fail instead of opening what it points at, and
        // `NONBLOCK` keeps a FIFO at the name from waiting for a writer; the handle is then checked
        // for its kind, its owner and its identity, so all three describe one object.
        let flags = OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        match rustix::fs::open(folder, flags, Mode::empty()) {
            Ok(file) => {
                let stat =
                    rustix::fs::fstat(&file).map_err(|e| storage("read", folder, e.into()))?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
                    return Err(not_a_folder(folder));
                }
                if !accepted_uid(stat.st_uid) {
                    return Err(private::owned_by_other(folder));
                }
                Ok(Some((stat.st_dev as u64, stat.st_ino as u64)))
            }
            Err(Errno::LOOP | Errno::NOTDIR) => Err(not_a_folder(folder)),
            // The folder cannot be opened. Its owner still decides whether that refusal is "another
            // user's" or a broken file system, and the identity is read from the entry itself, as
            // `lstat` does. The caller refuses the folder unless it is this user's or root's.
            Err(Errno::ACCESS | Errno::PERM) => {
                let meta = fs::symlink_metadata(folder).map_err(|e| storage("read", folder, e))?;
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(not_a_folder(folder));
                }
                if !owned_by_this_user(folder)
                    .map_err(|e| storage("read the owner of", folder, e))?
                {
                    return Err(private::owned_by_other(folder));
                }
                use std::os::unix::fs::MetadataExt;
                Ok(Some((meta.dev(), meta.ino())))
            }
            Err(e) => Err(storage("read", folder, e.into())),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use super::not_a_folder;
    use crate::Error;
    use crate::home::private_windows::{Checked, checked_folder};
    use crate::home::setup::private;
    use crate::home::storage;

    pub(super) fn check(folder: &Path) -> Result<Option<(u64, u64)>, Error> {
        match checked_folder(folder).map_err(|e| storage("read", folder, e))? {
            Checked::Folder { first, second } => Ok(Some((first, second))),
            Checked::NotAFolder => Err(not_a_folder(folder)),
            Checked::SomeoneElses => Err(private::owned_by_other(folder)),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use std::path::Path;

    use super::not_a_folder;
    use crate::Error;
    use crate::home::storage;

    pub(super) fn check(folder: &Path) -> Result<Option<(u64, u64)>, Error> {
        let meta = std::fs::symlink_metadata(folder).map_err(|e| storage("read", folder, e))?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(not_a_folder(folder));
        }
        Ok(None)
    }
}
