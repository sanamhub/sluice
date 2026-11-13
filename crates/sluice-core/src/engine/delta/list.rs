//! Building the signature list of one root, once, for every requester that asks for it.
//!
//! A list costs a read of the whole blob and a hash of every byte, so the first request for a root
//! starts one build and every other request for that root waits on it. The finished list is written
//! to a file in the send's own work folder, which goes with the send, and each requester is served
//! from that file. The work is not done on the async workers: the blob is read by an async task and
//! handed, through a bounded channel, to a blocking thread that cuts and hashes it, so a slow list
//! holds a thread of its own and no more than the channel's few pieces of memory.
//!
//! A list is built with this side's own frame limit, never a peer's, so what one requester says
//! about its limit cannot change what every other requester is given: a requester whose limit is
//! below the list is refused and the list stays.
//!
//! A build belongs to the requesters waiting for it. When the last of them has gone, its connection
//! closed, the build is told to stop and gives its permit back, so a peer that asks and leaves does
//! not keep a sender busy. A build that has read half of its blob keeps going and caches its list
//! instead: stopping it would waste what it did, and a peer that asks, leaves and asks again could
//! otherwise keep both permits hashing for ever without a list ever being finished.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use iroh::endpoint::Connection;
use iroh_blobs::{Hash, api::proto::BlobStatus, store::fs::FsStore};
use sluice_delta::{SMALLEST, SigList};
use tokio::sync::{Semaphore, mpsc, watch};

use super::{frame, params_for};
use pipe::{Reader, cut};

mod pipe;
use crate::engine::hello::MAX_FRAME_BYTES;

/// How many lists are built at once, across every root. A request past this is answered with
/// nothing and fetches plainly.
pub(super) const CONCURRENT: usize = 2;
/// How many pieces of a blob wait between the reader and the thread that hashes them.
const QUEUE: usize = 8;

/// A finished list: where its frame is, and the size of the blob it is about.
#[derive(Debug)]
pub(super) struct Built {
    pub(super) size: u64,
    path: PathBuf,
}

/// Where a build is.
#[derive(Clone)]
enum State {
    Building,
    /// Finished. `None` is a blob that has no list for a reason that does not change: it is too
    /// small, or no average fits it.
    Done(Option<Arc<Built>>),
    /// Stopped, refused for want of a permit, or failed on something that may not recur: a read, a
    /// write, a missing work folder. Nothing is cached, so the next request builds afresh.
    Gone,
}

/// How a build ended.
enum Outcome {
    Built(Built),
    /// No list, and none will ever be.
    Nothing,
    /// No list this time.
    Failed,
}

struct Flight {
    state: watch::Sender<State>,
    waiters: AtomicUsize,
    stop: AtomicBool,
    /// Names the files this flight writes, so a flight that was stopped cannot tear another's.
    id: u64,
    /// The bytes of the blob read so far and its size, once known.
    read: AtomicU64,
    size: AtomicU64,
}

impl Flight {
    /// Whether the build is past half of its blob.
    fn past_half(&self) -> bool {
        let size = self.size.load(Ordering::SeqCst);
        size > 0 && self.read.load(Ordering::SeqCst).saturating_mul(2) >= size
    }
}

/// The builds of one send.
pub(super) struct Lists {
    flights: Mutex<HashMap<Hash, Arc<Flight>>>,
    permits: Arc<Semaphore>,
    work: Mutex<Option<PathBuf>>,
    next: AtomicU64,
    /// What a test makes a build wait, so a slow list is reached through a real transfer: before it
    /// reads, for each block the reader sends, and for each block the hashing thread takes.
    #[cfg(test)]
    delay: Mutex<Delays>,
    /// How many builds have started, and how many stopped in the reader and in the hashing thread.
    #[cfg(test)]
    counts: Counts,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
pub(super) struct Delays {
    pub(super) before: std::time::Duration,
    pub(super) read: std::time::Duration,
    pub(super) cut: std::time::Duration,
}

#[cfg(test)]
#[derive(Default)]
struct Counts {
    started: AtomicUsize,
    read_stopped: AtomicUsize,
    cut_stopped: AtomicUsize,
}

/// One requester's place in a flight. Dropping it is leaving, and the last to leave stops the build
/// unless it is past half of its blob.
struct Waiter {
    lists: Arc<Lists>,
    hash: Hash,
    flight: Arc<Flight>,
}

impl Drop for Waiter {
    fn drop(&mut self) {
        let mut flights = self.lists.lock();
        if self.flight.waiters.fetch_sub(1, Ordering::SeqCst) == 1
            && matches!(*self.flight.state.borrow(), State::Building)
            && !self.flight.past_half()
        {
            self.flight.stop.store(true, Ordering::SeqCst);
            if flights
                .get(&self.hash)
                .is_some_and(|f| Arc::ptr_eq(f, &self.flight))
            {
                flights.remove(&self.hash);
            }
        }
    }
}

impl Lists {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            flights: Mutex::new(HashMap::new()),
            permits: Arc::new(Semaphore::new(CONCURRENT)),
            work: Mutex::new(None),
            next: AtomicU64::new(0),
            #[cfg(test)]
            delay: Mutex::new(Delays::default()),
            #[cfg(test)]
            counts: Counts::default(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Hash, Arc<Flight>>> {
        self.flights.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The folder lists are written in. Without one no list is built.
    pub(super) fn set_work(&self, dir: PathBuf) {
        *self.work.lock().unwrap_or_else(|p| p.into_inner()) = Some(dir);
    }

    /// How many permits are free, for a test that checks a build gave its permit back.
    #[cfg(test)]
    pub(super) fn free_permits(&self) -> usize {
        self.permits.available_permits()
    }

    /// How many builds have started. A test only reaches this.
    #[cfg(test)]
    pub(super) fn builds(&self) -> usize {
        self.counts.started.load(Ordering::SeqCst)
    }

    /// How many builds stopped in the reader and in the hashing thread. A test only reaches this.
    #[cfg(test)]
    pub(super) fn stopped_in(&self) -> (usize, usize) {
        (
            self.counts.read_stopped.load(Ordering::SeqCst),
            self.counts.cut_stopped.load(Ordering::SeqCst),
        )
    }

    /// Sets what each build waits. A test only reaches this.
    #[cfg(test)]
    pub(super) fn delay_builds(&self, delays: Delays) {
        *self.delay.lock().unwrap_or_else(|p| p.into_inner()) = delays;
    }

    /// The frame that carries the list of `hash`, and the size of the blob, for a requester whose
    /// frames may be `frame_limit` bytes. `None` is nothing to say: no permit was free, the blob is
    /// too small or too large, the build failed, the list is longer than the requester takes, or
    /// the requester's connection closed first. A requester that cannot take the list leaves it as
    /// it is for the next.
    pub(super) async fn get(
        self: &Arc<Self>,
        store: &FsStore,
        hash: Hash,
        frame_limit: u32,
        conn: &Connection,
    ) -> Option<(Vec<u8>, u64)> {
        let (waiter, mut state) = self.join(store, hash);
        let built = loop {
            match &*state.borrow_and_update() {
                State::Done(built) => break built.clone()?,
                State::Gone => return None,
                State::Building => {}
            }
            tokio::select! {
                changed = state.changed() => {
                    if changed.is_err() {
                        return None;
                    }
                }
                _ = conn.closed() => return None,
            }
        };
        drop(waiter);
        let bytes = tokio::fs::read(&built.path).await.ok()?;
        (bytes.len() as u64 <= u64::from(frame_limit)).then_some((bytes, built.size))
    }

    /// Joins the build of `hash` that is under way, or starts one.
    fn join(self: &Arc<Self>, store: &FsStore, hash: Hash) -> (Waiter, watch::Receiver<State>) {
        let mut flights = self.lock();
        if let Some(flight) = flights.get(&hash)
            && !flight.stop.load(Ordering::SeqCst)
        {
            flight.waiters.fetch_add(1, Ordering::SeqCst);
            let flight = Arc::clone(flight);
            let state = flight.state.subscribe();
            return (self.waiter(hash, flight), state);
        }
        let (state, receiver) = watch::channel(State::Building);
        let flight = Arc::new(Flight {
            state,
            waiters: AtomicUsize::new(1),
            stop: AtomicBool::new(false),
            id: self.next.fetch_add(1, Ordering::SeqCst),
            read: AtomicU64::new(0),
            size: AtomicU64::new(0),
        });
        flights.insert(hash, Arc::clone(&flight));
        drop(flights);
        let lists = Arc::clone(self);
        let (store, started) = (store.clone(), Arc::clone(&flight));
        tokio::spawn(async move {
            lists.run(store, hash, started).await;
        });
        (self.waiter(hash, flight), receiver)
    }

    fn waiter(self: &Arc<Self>, hash: Hash, flight: Arc<Flight>) -> Waiter {
        Waiter {
            lists: Arc::clone(self),
            hash,
            flight,
        }
    }

    /// Runs one build and says how it ended. A build that was stopped, refused or failed is
    /// forgotten, so the next request starts afresh; one that finished is kept for every other
    /// requester.
    async fn run(self: Arc<Self>, store: FsStore, hash: Hash, flight: Arc<Flight>) {
        let outcome = self.build(&store, hash, &flight).await;
        let stopped = flight.stop.load(Ordering::SeqCst);
        let state = match outcome {
            Some(Outcome::Built(built)) if !stopped => State::Done(Some(Arc::new(built))),
            Some(Outcome::Nothing) if !stopped => State::Done(None),
            _ => {
                let mut flights = self.lock();
                if flights.get(&hash).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
                    flights.remove(&hash);
                }
                State::Gone
            }
        };
        flight.state.send_replace(state);
    }

    /// Reads the blob, cuts it and hashes the chunks, and writes the frame. `None` is no permit.
    async fn build(&self, store: &FsStore, hash: Hash, flight: &Arc<Flight>) -> Option<Outcome> {
        let _permit = Arc::clone(&self.permits).try_acquire_owned().ok()?;
        #[cfg(test)]
        self.counts.started.fetch_add(1, Ordering::SeqCst);
        let Some(work) = self.work.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
            return Some(Outcome::Failed);
        };
        let size = match store.blobs().status(hash).await {
            Ok(BlobStatus::Complete { size }) => size,
            _ => return Some(Outcome::Failed),
        };
        if size < SMALLEST {
            return Some(Outcome::Nothing);
        }
        // This side's own frame limit decides the average, so every requester is given the same list.
        let Some(params) = params_for(size, MAX_FRAME_BYTES) else {
            return Some(Outcome::Nothing);
        };
        flight.size.store(size, Ordering::SeqCst);
        #[cfg(test)]
        let delays = *self.delay.lock().unwrap_or_else(|p| p.into_inner());
        #[cfg(test)]
        {
            let until = std::time::Instant::now() + delays.before;
            while std::time::Instant::now() < until && !flight.stop.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        #[cfg(test)]
        let cut_delay = delays.cut;
        #[cfg(not(test))]
        let cut_delay = std::time::Duration::ZERO;
        let hasher = {
            let flight = Arc::clone(flight);
            tokio::task::spawn_blocking(move || cut(rx, params, &flight.stop, cut_delay))
        };
        #[cfg(test)]
        let read_delay = delays.read;
        #[cfg(not(test))]
        let read_delay = std::time::Duration::ZERO;
        let reader = Reader {
            store,
            hash,
            flight,
            delay: read_delay,
        };
        let read_ok = reader.run(tx).await;
        #[cfg(test)]
        if !read_ok && flight.stop.load(Ordering::SeqCst) {
            self.counts.read_stopped.fetch_add(1, Ordering::SeqCst);
        }
        let found = hasher.await.ok().flatten();
        #[cfg(test)]
        if found.is_none() && flight.stop.load(Ordering::SeqCst) {
            self.counts.cut_stopped.fetch_add(1, Ordering::SeqCst);
        }
        let (entries, read) = found?;
        if !read_ok || read != size || flight.stop.load(Ordering::SeqCst) {
            return Some(Outcome::Failed);
        }
        let Ok(list) = SigList::new(params.avg(), entries) else {
            return Some(Outcome::Failed);
        };
        Some(match self.write(&work, hash, flight.id, &list).await {
            Some(path) => Outcome::Built(Built { size, path }),
            None => Outcome::Failed,
        })
    }

    /// Writes the frame to a name of this flight's own and renames it into place, so a flight that
    /// is stopped or that loses a race never leaves a half-written file at the name a list is read
    /// from. Returns where the list is.
    async fn write(
        &self,
        work: &std::path::Path,
        hash: Hash,
        id: u64,
        list: &SigList,
    ) -> Option<PathBuf> {
        let path = work.join(format!("delta-{hash}.sig"));
        let temporary = work.join(format!("delta-{hash}.{id}.tmp"));
        tokio::fs::create_dir_all(work).await.ok()?;
        tokio::fs::write(&temporary, frame(&list.encode()))
            .await
            .ok()?;
        if tokio::fs::rename(&temporary, &path).await.is_err() {
            // Another flight put the same list there first: the bytes are the same.
            let _ = tokio::fs::remove_file(&temporary).await;
            tokio::fs::metadata(&path).await.ok()?;
        }
        Some(path)
    }
}
