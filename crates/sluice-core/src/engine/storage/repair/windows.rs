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
    time::Duration,
};

use windows_sys::Win32::{
    Foundation::ERROR_SHARING_VIOLATION,
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
};

use super::{alien, changed, sweep_paths};
use crate::{
    Error,
    engine::storage::{FolderId, HOLD_DELAYS, storage},
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
            Err(error) => return Err(open_error(partial, error)),
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
            Err(error) => return Err(open_error(&data, error)),
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
///
/// A handle another program holds *with* delete access is the one thing that stops this open: the
/// share mode here does not allow that access, so the open fails with a sharing violation. A scanner
/// or an indexer holds a folder that way for a moment while it looks at it, so the open is tried
/// again with the waits a removal gets, and a program that keeps the folder for longer is named in
/// the error rather than reported as an access denial.
fn open_held(path: &Path) -> io::Result<fs::File> {
    match open_once(path) {
        Ok(handle) => Ok(handle),
        Err(error) if !shared(&error) => Err(error),
        Err(mut last) => {
            for delay in HOLD_DELAYS {
                std::thread::sleep(Duration::from_millis(delay));
                match open_once(path) {
                    Ok(handle) => return Ok(handle),
                    Err(error) => last = error,
                }
            }
            Err(last)
        }
    }
}

fn open_once(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

/// Whether the open failed because a handle that another program holds has delete access, which
/// this open does not share.
fn shared(error: &io::Error) -> bool {
    error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32)
}

/// The error for a folder the repair could not open. A folder another program holds is not a fault
/// of the store: its bytes are as they were, and running the receive again can work once that
/// program has let go.
fn open_error(path: &Path, error: io::Error) -> Error {
    if shared(&error) {
        return Error::Storage(format!(
            "{} is in use by another program; the receive can be run again",
            path.display()
        ));
    }
    storage("open", path, error)
}
