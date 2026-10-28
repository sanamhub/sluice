//! The store folder held open without following a link, with the identity read from that handle.
//!
//! A folder cannot be listed through a handle without a call this tree does not make, so the files
//! are read by path after the handle has named a real folder and `data` has been checked for a
//! reparse point. The identity of the folder is checked again when the sweep is over, so a folder
//! swapped in under the name is refused rather than trusted.

use std::{
    fs, io,
    path::{Path, PathBuf},
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
        let handle = match private_windows::open_entry(partial) {
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
        let held = match private_windows::open_entry(&data) {
            Ok(handle) => handle,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(storage("open", &data, error)),
        };
        let (reparse, folder) =
            private_windows::handle_kind(&held).map_err(|error| storage("read", &data, error))?;
        if reparse || !folder {
            return Err(alien(partial));
        }
        sweep_paths(&data, partial)
    }
}
