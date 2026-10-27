//! The store a receive reads the hash sequence and the manifest into before it may claim its own.

use std::{env, path::PathBuf};

use iroh_blobs::store::fs::FsStore;

use super::{remove_quietly, temp_dir};
use crate::Error;

/// The store a receive reads the hash sequence and the manifest into. Both are needed before the
/// checks, which come before this receive may claim a partial store beside the output. It lives in
/// the system temporary folder, which is this user's on every platform we support, and it is
/// removed however the receive ends.
pub struct MetaStore {
    dir: PathBuf,
    /// The store inside the folder.
    pub store: FsStore,
}

impl MetaStore {
    pub async fn open() -> Result<Self, Error> {
        let dir = temp_dir(&env::temp_dir())?;
        let store = match FsStore::load(dir.join("store")).await {
            Ok(store) => store,
            Err(e) => {
                remove_quietly(&dir);
                return Err(Error::Storage(format!(
                    "cannot open a temporary store: {}",
                    crate::plain(&e)
                )));
            }
        };
        Ok(Self { dir, store })
    }

    /// Closes the store and removes the folder.
    pub async fn close(self) -> Result<(), Error> {
        self.store.shutdown().await.map_err(|e| {
            Error::Storage(format!(
                "cannot close a temporary store: {}",
                crate::plain(&e)
            ))
        })
    }
}

impl Drop for MetaStore {
    fn drop(&mut self) {
        remove_quietly(&self.dir);
    }
}
