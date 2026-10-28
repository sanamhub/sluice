//! The store a receive reads the hash sequence and the manifest into before it may claim its own.

use std::path::Path;

use super::{Collector, Store, TempStore};
use crate::{Error, Home};

/// The store a receive reads the hash sequence and the manifest into. Both are needed before the
/// checks, which come before this receive may claim a partial store beside the output. It lives in
/// the home's `tmp` folder, which is private to this user: the system temporary folder is shared on
/// Unix, so another local user could read the names and sizes of the manifest there. A kill leaves
/// the folder behind for the sweep of the next command.
pub struct MetaStore {
    /// The store and the folder it lives in, which goes only after the store is closed.
    store: TempStore,
}

impl MetaStore {
    pub async fn open() -> Result<Self, Error> {
        let parent = Home::temporary_store()?;
        Self::open_in(&parent).await
    }

    /// [`open`] in a folder the caller names, which must already be private.
    pub(super) async fn open_in(parent: &Path) -> Result<Self, Error> {
        let store = TempStore::open(parent, Collector::Off).await.map_err(|e| {
            Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::plain(&e)
            ))
        })?;
        Ok(Self { store })
    }

    /// The store, for the fetches that fill it.
    pub(crate) fn store(&self) -> &Store {
        self.store.store()
    }

    /// The folder this store owns, so a test can see where it went.
    #[cfg(test)]
    pub(super) fn dir(&self) -> &Path {
        self.store.path()
    }

    /// Closes the store and removes the folder, in that order.
    pub async fn close(self) -> Result<(), Error> {
        self.store.close().await
    }
}
