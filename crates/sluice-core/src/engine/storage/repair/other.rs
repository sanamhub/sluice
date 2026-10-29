//! The store folder as a path, for a platform with no call to open it as a handle. Nothing here is
//! compiled on Unix or Windows; it keeps the module complete where those calls do not exist.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::{changed, sweep_paths};
use crate::{
    Error,
    engine::storage::{FolderId, identify, storage},
};

pub(super) struct Dir {
    path: PathBuf,
}

impl Dir {
    pub(super) fn open(partial: &Path) -> Result<Option<Self>, Error> {
        match fs::metadata(partial) {
            Ok(meta) if meta.is_dir() => Ok(Some(Self {
                path: partial.to_owned(),
            })),
            Ok(_) => Err(changed(partial)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(storage("open", partial, error)),
        }
    }

    pub(super) fn identity(&self) -> Result<Option<FolderId>, Error> {
        identify(&self.path)
    }

    pub(super) fn sweep(self, partial: &Path) -> Result<(), Error> {
        sweep_paths(&partial.join("data"), partial)
    }
}
