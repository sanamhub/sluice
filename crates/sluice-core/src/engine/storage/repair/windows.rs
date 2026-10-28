//! The store folder held open without following a link, with the identity read from that handle.
//!
//! A folder cannot be listed through a handle without a call this tree does not make, so the files
//! are read by path after the handle has named a real folder and `data` has been checked for a
//! reparse point. The identity of the folder is checked again when the sweep is over, so a folder
//! swapped in under the name is refused rather than trusted. Both handles also withhold delete
//! sharing, so neither name can be renamed away and given a junction while the sweep is running:
//! the sweep deletes and renames files by path, and a folder of someone else's choosing is not the
//! folder that was checked.

use std::{
    fs, io,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
    FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use super::{alien, changed, sweep_paths};
use crate::{
    Error,
    engine::storage::{FolderId, storage},
    home::private_windows,
};

pub(super) struct Dir {
    handle: fs::File,
    path: PathBuf,
}

impl Dir {
    /// Opens `partial` as a folder, without following a link at its name.
    pub(super) fn open(partial: &Path) -> Result<Option<Self>, Error> {
        let handle = match open_held(partial) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(storage("open", partial, error)),
        };
        let (reparse, folder) = private_windows::handle_kind(&handle)
            .map_err(|error| storage("read", partial, error))?;
        if reparse || !folder {
            return Err(changed(partial));
        }
        Ok(Some(Self {
            handle,
            path: partial.to_owned(),
        }))
    }

    /// The identity of the folder this handle names, and not of the name it was opened through.
    pub(super) fn identity(&self) -> Result<Option<FolderId>, Error> {
        let (first, second) = private_windows::handle_id(&self.handle)
            .map_err(|error| storage("read", &self.path, error))?;
        Ok(Some(FolderId::of((first, second))))
    }

    pub(super) fn sweep(self, partial: &Path) -> Result<(), Error> {
        let data = partial.join("data");
        let held = match open_held(&data) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage("open", &data, error)),
        };
        let (reparse, folder) =
            private_windows::handle_kind(&held).map_err(|error| storage("read", &data, error))?;
        if reparse || !folder {
            return Err(alien(partial));
        }
        #[cfg(test)]
        super::tests::hold::wait(partial);
        sweep_paths(&data, partial)
    }
}

/// A handle to the folder that keeps its name from being renamed or removed while the repair works.
/// The rest of the home checks a folder and closes the handle at once, and every one of those
/// callers may rename or remove what it checked straight afterwards, so they keep the default
/// sharing. This handle is held for the whole sweep, so it takes read and write sharing and not
/// delete sharing: the store's files stay readable and writable, and the folder itself cannot move.
///
/// The access rights matter as much as the sharing: a handle opened for the directory's owner and
/// security only is not one the file system counts when another open asks to delete or rename the
/// folder, and the move goes through. Listing the folder is what makes the handle hold its name.
fn open_held(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
