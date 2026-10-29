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
    /// The handle every method uses. It is not an `Option`, so no method has to say what a store
    /// that was already closed would mean: closing consumes the value.
    fs: FsStore,
    /// A second handle of the same store, handed to the next command unless this one closed it. It
    /// is kept apart from `fs` so the deref path never meets an empty guard.
    hand: HandOff,
}

impl Store {
    /// Opens the store at `root`, after closing any store an interrupted command left open: a
    /// resume opens the same folder, and its database has to be free first.
    pub async fn open(root: &Path, collector: Collector) -> Result<Self, Error> {
        settle().await;
        let fs = open_store(root, collector).await?;
        Ok(Self {
            hand: HandOff(Some(fs.clone())),
            fs,
        })
    }

    /// A second handle, for a task that serves or writes while this guard is alive.
    pub fn handle(&self) -> FsStore {
        self.fs.clone()
    }

    /// Closes the store, so its database is free before this returns. It consumes the value, so
    /// nothing can use a handle to a store it already closed.
    pub async fn close(mut self) -> Result<(), Error> {
        self.close_in_place().await
    }

    /// [`close`] on a value that is owned by something with a drop of its own, which cannot give the
    /// value up but can still close it.
    pub(crate) async fn close_in_place(&mut self) -> Result<(), Error> {
        self.hand.0 = None;
        self.fs
            .shutdown()
            .await
            .map_err(|e| Error::Storage(format!("cannot close the store: {}", crate::plain(&e))))
    }

    /// Stops this value handing the store on, for a caller that gives the store to something else
    /// that closes it, or that already handed it on.
    pub(crate) fn disarm(&mut self) {
        self.hand.0 = None;
    }
}

impl Deref for Store {
    type Target = FsStore;

    fn deref(&self) -> &FsStore {
        &self.fs
    }
}

/// A handle that is handed to the next command when it is dropped, unless it was taken.
struct HandOff(Option<FsStore>);

impl Drop for HandOff {
    /// A command that was cancelled has no moment to await a close. The store goes to the next
    /// command in this process, which closes it before it opens its own.
    fn drop(&mut self) {
        if let Some(store) = self.0.take() {
            unclosed().push(Unclosed {
                store: Some(store),
                dir: None,
            });
        }
    }
}

/// A store a cancelled command could not close, with the folder that has to go after it. The folder
/// is kept with the store and not removed on its own: a folder removed while the database is still
/// open fails on Windows and leaves the folder behind.
struct Unclosed {
    store: Option<FsStore>,
    dir: Option<super::Temp>,
}

/// Stores a cancelled command could not close. Closing one is a message to its actor and a wait
/// for the answer; a dropped future has nowhere to wait, so the work is handed on.
static UNCLOSED: Mutex<Vec<Unclosed>> = Mutex::new(Vec::new());

/// Serialises the closes, so an open that starts while another is closing a store waits for the
/// close to finish instead of finding the parked list already empty and the database still held.
static SETTLING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn unclosed() -> std::sync::MutexGuard<'static, Vec<Unclosed>> {
    UNCLOSED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Hands a store and the folder it lives in to the next command, when the command that held them
/// was cancelled.
///
/// Where a runtime can still run the close, it is done at once, in the order that matters: the
/// database is closed and the folder is removed after it. The close takes the same lock an open
/// takes, so an open that starts while this close is running waits for it instead of finding the
/// parked list already empty and the database still held. The folder goes after the lock is
/// released and on a blocking thread, because a removal can wait for a file that another process
/// still holds and an open has no reason to wait for that. Where a runtime cannot run the close,
/// the pair waits for the next command that opens a store, which does the same steps in the same
/// order. A process that ends before either leaves a folder the sweep of temporary folders removes.
pub(crate) fn hand_on(store: Option<FsStore>, dir: Option<super::Temp>) {
    match (store, dir) {
        (Some(store), Some(dir)) => {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                // Counted before the task is spawned, so the end of the command waits for a close
                // and a removal that have not been scheduled yet.
                let doing = super::removals::Doing::begin();
                handle.spawn(async move {
                    let _doing = doing;
                    {
                        let _settled = SETTLING.lock().await;
                        let _ = store.shutdown().await;
                    }
                    dir.remove_apart();
                });
            } else {
                unclosed().push(Unclosed {
                    store: Some(store),
                    dir: Some(dir),
                });
            }
        }
        (store, dir) => unclosed().push(Unclosed { store, dir }),
    }
}

/// Closes every store an interrupted command left open, and removes the folder of one that was
/// living in a temporary one. The folders go after the lock and each on its own blocking thread:
/// an open that follows waits for the closes it can see, not for a folder a held file keeps.
async fn settle() {
    let mut dirs = Vec::new();
    {
        let _settled = SETTLING.lock().await;
        let handing = std::mem::take(&mut *unclosed());
        for Unclosed { store, dir } in handing {
            if let Some(store) = store {
                let _ = store.shutdown().await;
            }
            dirs.extend(dir);
        }
    }
    for dir in dirs {
        dir.remove_apart();
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
