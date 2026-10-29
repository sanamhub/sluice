//! Where one send keeps the bytes it copies: a store of its own in a private folder of the home's
//! temporary area, removed when the send ends.
//!
//! A send imports regular files where they live and copies nothing of them, so its store holds only
//! what was copied: standard input, the packs of small files, the manifest and the hash sequence,
//! and the sealed segments of a live stream. That is the send's own business and nobody else's, so
//! it goes in a folder of its own that no other command reads and that is removed when the send
//! stops, however it stops.

use std::fs;
use std::path::Path;

use crate::engine::storage::{Collector, Store, Temp, TempStore};
use crate::{Error, Home};

/// The store one send copies into.
pub(crate) struct Scratch {
    store: TempStore,
}

impl Scratch {
    /// A new store for one send, with the collector the caller chose: only a live send's bytes are
    /// throwaway while it runs, so only that one collects.
    ///
    /// The home's `blobs` folder is removed here rather than in every other command: a send is what
    /// used to fill it, and no other command reads it any more.
    pub(crate) async fn open(home: &Home, collector: Collector) -> Result<Self, Error> {
        let root = home.path().to_path_buf();
        let _ = tokio::task::spawn_blocking(move || forget_home_store(&root)).await;
        let parent = home.temporary_folder()?;
        Ok(Self {
            store: TempStore::open(&parent, collector).await?,
        })
    }

    /// The store, for the import and the tickets.
    pub(crate) fn store(&self) -> &Store {
        self.store.store()
    }

    /// The folder, for a temporary file that belongs to this send.
    pub(crate) fn path(&self) -> &Path {
        self.store.path()
    }

    /// Closes the store and removes the folder.
    pub(crate) async fn close(self) -> Result<(), Error> {
        self.store.close().await
    }

    /// Gives the store to a provider, which closes it when it stops, and returns the folder so it
    /// can be removed after that.
    pub(crate) fn release(self) -> Option<Temp> {
        self.store.release()
    }
}

/// Removes the `blobs` folder an older home holds, when it is the folder this tree makes. A send
/// used to keep its imports there, and a home that sent anything may still carry them; nothing
/// reads that folder now, and it holds nothing a user can name. It is removed on a blocking thread
/// because a home can hold many files and the send has other work to do.
fn forget_home_store(root: &Path) {
    let blobs = root.join("blobs");
    let Ok(meta) = fs::symlink_metadata(&blobs) else {
        return;
    };
    // A link is not a folder this tree made, and removing through one would empty whatever it leads
    // to. It is left where it is for its owner to look at.
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return;
    }
    let _ = fs::remove_dir_all(&blobs);
}

#[cfg(test)]
mod tests {
    use super::Scratch;
    use crate::Home;
    use crate::engine::storage::{Collector, TEMP_PREFIX};

    /// A send keeps its store in a folder with the name the sweep of temporary folders removes, so a
    /// send that was killed before it could remove its own does not keep the bytes for ever.
    #[tokio::test]
    async fn a_send_keeps_its_store_in_a_folder_the_sweep_looks_at() {
        let dir = tempfile::tempdir().unwrap();
        let home = Home::open(&dir.path().join("home")).unwrap();
        let scratch = Scratch::open(&home, Collector::Off).await.unwrap();
        let name = scratch
            .path()
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        assert!(name.starts_with(TEMP_PREFIX), "{name}");
        assert_eq!(
            scratch.path().parent(),
            Some(home.temporary_folder().unwrap().as_path()),
            "the store is not in the home's temporary area"
        );
        scratch.close().await.unwrap();
        assert!(!home.path().join("tmp").join(name).exists());
    }
}
