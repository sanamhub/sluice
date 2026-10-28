//! Opening a store, choosing whether it collects, and closing it on every way out of a command.

use std::{ops::Deref, path::Path, sync::Mutex, time::Duration};

use iroh_blobs::store::{
    GcConfig,
    fs::{FsStore, options::Options},
};

use crate::Error;

/// How often the store's collector runs. A live stream drops each segment's tag as soon as the
/// segment is written out, and this is the longest the bytes can stay after that; it is also the
/// longest a segment is kept for a reader whose request is still on its way. Ten seconds is a few
/// segments at the rates a live stream is sealed at and far longer than a reader needs to have its
/// request handled.
pub const GC_INTERVAL: Duration = Duration::from_secs(10);

/// Whether a store runs the collector that frees a blob no tag holds.
///
/// Every caller states it, so a store cannot be opened without saying whether its blobs may be
/// freed. Only a store whose blobs are throwaway collects: the live send and the live receive keep
/// each segment while it is being served or written, and free it afterwards. Every other store is a
/// resume store, a pack a range reads, or a manifest a check depends on, and has to keep its bytes
/// until the store is done or discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Collector {
    /// A blob stays only while a tag or a temporary tag holds it.
    On,
    /// Every blob stays until the store is closed or its folder is removed.
    Off,
}

/// A store that is closed on every way out of the command that opened it.
///
/// A store holds its database open until its actor is asked to close it, and a command that
/// returned early, or that was cancelled, used to leave it open: the actor kept the database
/// locked, so a resume in the same process could not open the store again. [`Store`] closes the
/// database wherever a command can await one, and hands a store it could not close to the next
/// command that opens one.
pub struct Store {
    inner: Option<FsStore>,
}

impl Store {
    /// Opens the store at `root`, after closing any store an interrupted command left open: a
    /// resume opens the same folder, and its database has to be free first.
    pub async fn open(root: &Path, collector: Collector) -> Result<Self, Error> {
        settle().await;
        Ok(Self {
            inner: Some(open_store(root, collector).await?),
        })
    }

    /// The store, for a call that needs the handle itself.
    pub fn fs(&self) -> &FsStore {
        self.inner
            .as_ref()
            .expect("a store is only gone once it was closed")
    }

    /// A second handle, for a task that serves or writes while this guard is alive.
    pub fn handle(&self) -> FsStore {
        self.fs().clone()
    }

    /// Gives the store up without closing it, for a value that closes it on its own way out.
    pub fn into_fs(mut self) -> FsStore {
        self.inner
            .take()
            .expect("a store is only gone once it was closed")
    }

    /// Closes the store, so its database is free before this returns. A second call does nothing.
    pub async fn close(&mut self) -> Result<(), Error> {
        let Some(store) = self.inner.take() else {
            return Ok(());
        };
        store
            .shutdown()
            .await
            .map_err(|e| Error::Storage(format!("cannot close the store: {}", crate::plain(&e))))
    }
}

impl Deref for Store {
    type Target = FsStore;

    fn deref(&self) -> &FsStore {
        self.fs()
    }
}

impl Drop for Store {
    /// A command that was cancelled has no moment to await a close. The store goes to the next
    /// command in this process, which closes it before it opens its own.
    fn drop(&mut self) {
        if let Some(store) = self.inner.take() {
            unclosed().push(store);
        }
    }
}

/// Stores a cancelled command could not close. Closing one is a message to its actor and a wait
/// for the answer; a dropped future has nowhere to wait, so the work is handed on.
static UNCLOSED: Mutex<Vec<FsStore>> = Mutex::new(Vec::new());

fn unclosed() -> std::sync::MutexGuard<'static, Vec<FsStore>> {
    UNCLOSED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Closes every store an interrupted command left open.
async fn settle() {
    let stores = std::mem::take(&mut *unclosed());
    for store in stores {
        let _ = store.shutdown().await;
    }
}

/// Opens the store at `root` with the collector the caller chose.
async fn open_store(root: &Path, collector: Collector) -> Result<FsStore, Error> {
    let interval = match collector {
        Collector::On => Some(gc_interval()),
        Collector::Off => None,
    };
    let mut options = Options::new(root);
    options.gc = interval.map(|interval| GcConfig {
        interval,
        add_protected: None,
    });
    FsStore::load_with_opts(root.join("blobs.db"), options)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store at {}: {e}", root.display())))
}

/// How often a collector sweeps. A unit test sweeps sooner so it does not wait ten seconds for the
/// first sweep; a command always sweeps at [`GC_INTERVAL`].
#[cfg(test)]
fn gc_interval() -> Duration {
    Duration::from_millis(100)
}

#[cfg(not(test))]
fn gc_interval() -> Duration {
    GC_INTERVAL
}

/// [`open_store`] with the collector's interval as an argument, so a test can make one sweep
/// happen sooner than the constant does.
#[cfg(test)]
pub(crate) async fn open_store_after(root: &Path, interval: Duration) -> Result<FsStore, Error> {
    let mut options = Options::new(root);
    options.gc = Some(GcConfig {
        interval,
        add_protected: None,
    });
    FsStore::load_with_opts(root.join("blobs.db"), options)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store at {}: {e}", root.display())))
}
