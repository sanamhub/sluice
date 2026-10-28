//! A store in a private temporary folder, closed before the folder goes.
//!
//! A folder removed while a store's database is still open fails on Windows and leaves the folder
//! behind, so the store and the folder that holds it are one value with one order: the database is
//! closed first and the folder removed after. A command that is cancelled cannot await either, so
//! the pair is handed to the next command, which closes the store and then removes the folder.

use std::path::Path;

use super::{Collector, Store, Temp, open, temp_dir};
use crate::Error;

/// A store and the folder it lives in.
pub struct TempStore {
    /// Declared first so it is dropped first: a handler that runs on the drop of this value closes
    /// the store before it removes the folder.
    store: Store,
    dir: Option<Temp>,
}

impl TempStore {
    /// Makes a folder in `parent` and opens a store inside it.
    pub async fn open(parent: &Path, collector: Collector) -> Result<Self, Error> {
        let dir = temp_dir(parent)?;
        let store = Store::open(&dir.path().join("store"), collector).await?;
        Ok(Self {
            store,
            dir: Some(dir),
        })
    }

    /// The store inside the folder.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The folder the store lives in.
    pub fn path(&self) -> &Path {
        self.dir.as_ref().map_or_else(|| Path::new(""), Temp::path)
    }

    /// Closes the store and then removes the folder.
    pub async fn close(mut self) -> Result<(), Error> {
        let closed = self.store.close_in_place().await;
        self.dir.take();
        closed
    }

    /// Gives the folder to a caller that removes it after the store's new owner closed it. Used
    /// where the store goes to a provider, which closes it when it stops serving.
    pub fn release(mut self) -> Option<Temp> {
        self.store.disarm();
        self.dir.take()
    }
}

impl Drop for TempStore {
    /// A cancelled command has no moment to await the close. The store and the folder go to the
    /// next command together, which closes the store and then removes the folder, and the folder is
    /// not removed under a database that is still open.
    fn drop(&mut self) {
        if let Some(dir) = self.dir.take() {
            let store = self.store.handle();
            self.store.disarm();
            open::hand_on(Some(store), Some(dir));
        }
    }
}
