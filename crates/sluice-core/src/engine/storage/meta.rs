//! The store a receive reads the hash sequence and the manifest into before it may claim its own.

use std::path::Path;

use super::{Store, Temp, temp_dir};
use crate::{Error, Home};

/// The store a receive reads the hash sequence and the manifest into. Both are needed before the
/// checks, which come before this receive may claim a partial store beside the output. It lives in
/// the home's `tmp` folder, which is private to this user: the system temporary folder is shared on
/// Unix, so another local user could read the names and sizes of the manifest there. A kill leaves
/// the folder behind for the sweep of the next command.
pub struct MetaStore {
    _dir: Temp,
    /// The store inside the folder.
    pub store: Store,
}

impl MetaStore {
    pub async fn open() -> Result<Self, Error> {
        let parent = Home::temporary_store()?;
        Self::open_in(&parent).await
    }

    /// [`open`] in a folder the caller names, which must already be private.
    pub(super) async fn open_in(parent: &Path) -> Result<Self, Error> {
        let dir = temp_dir(parent)?;
        let store = match Store::open(&dir.path().join("store"), super::Collector::Off).await {
            Ok(store) => store,
            Err(e) => {
                return Err(Error::Storage(format!(
                    "cannot open a temporary store: {}",
                    crate::plain(&e)
                )));
            }
        };
        Ok(Self { _dir: dir, store })
    }

    /// The folder this store owns, so a test can see where it went.
    #[cfg(test)]
    pub(super) fn dir(&self) -> &Path {
        self._dir.path()
    }

    /// Closes the store and removes the folder.
    pub async fn close(mut self) -> Result<(), Error> {
        self.store.close().await
    }
}
