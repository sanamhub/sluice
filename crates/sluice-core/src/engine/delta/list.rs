//! Building the signature list of one root, once, for every requester that asks for it.
//!
//! A list costs a read of the whole blob and a hash of every byte, so the first request for a root
//! starts one build and every other request for that root waits on it. The finished list is written
//! to a file in the send's own work folder, which goes with the send, and each requester is served
//! from that file. The work is not done on the async workers: the blob is read by an async task and
//! handed, through a bounded channel, to a blocking thread that cuts and hashes it, so a slow list
//! holds a thread of its own and no more than the channel's few pieces of memory.
//!
//! A build belongs to the requesters waiting for it. When the last of them has gone, its connection
//! closed, the build is told to stop and gives its permit back, so a peer that asks and leaves does
//! not keep a sender busy.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use iroh::endpoint::Connection;
use iroh_blobs::{Hash, api::proto::BlobStatus, store::fs::FsStore};
use sluice_delta::{Entry, MAX_ENTRIES, SMALLEST, SigList, StreamCut};
use tokio::sync::{Semaphore, mpsc, watch};

use super::{frame, params_for};

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
    Done(Option<Arc<Built>>),
    /// Stopped, or refused for want of a permit. Nothing is cached, so the next request tries again.
    Gone,
}

struct Flight {
    state: watch::Sender<State>,
    waiters: AtomicUsize,
    stop: AtomicBool,
}

/// The builds of one send.
pub(super) struct Lists {
    flights: Mutex<HashMap<Hash, Arc<Flight>>>,
    permits: Arc<Semaphore>,
    work: Mutex<Option<PathBuf>>,
    /// How long a test makes each build wait before it reads, so a slow list is reached through a
    /// real transfer.
    #[cfg(test)]
    delay: Mutex<std::time::Duration>,
    /// How many builds have started, for a test that checks a root is built once.
    #[cfg(test)]
    started: AtomicUsize,
}

/// One requester's place in a flight. Dropping it is leaving, and the last to leave stops the build.
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
            #[cfg(test)]
            delay: Mutex::new(std::time::Duration::ZERO),
            #[cfg(test)]
            started: AtomicUsize::new(0),
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
        self.started.load(Ordering::SeqCst)
    }

    /// Makes each build wait `by` before it reads. A test only reaches this.
    #[cfg(test)]
    pub(super) fn delay_builds(&self, by: std::time::Duration) {
        *self.delay.lock().unwrap_or_else(|p| p.into_inner()) = by;
    }

    /// The frame that carries the list of `hash`, and the size of the blob, for a requester whose
    /// frames may be `frame_limit` bytes. `None` is nothing to say: no permit was free, the blob is
    /// too small or too large, the build failed, or the requester's connection closed first.
    pub(super) async fn get(
        self: &Arc<Self>,
        store: &FsStore,
        hash: Hash,
        frame_limit: u32,
        conn: &Connection,
    ) -> Option<(Vec<u8>, u64)> {
        let (waiter, mut state) = self.join(store, hash, frame_limit);
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
    fn join(
        self: &Arc<Self>,
        store: &FsStore,
        hash: Hash,
        frame_limit: u32,
    ) -> (Waiter, watch::Receiver<State>) {
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
        });
        flights.insert(hash, Arc::clone(&flight));
        drop(flights);
        let lists = Arc::clone(self);
        let (store, started) = (store.clone(), Arc::clone(&flight));
        tokio::spawn(async move {
            lists.run(store, hash, frame_limit, started).await;
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

    /// Runs one build and says how it ended. A build that was stopped or refused is forgotten, so
    /// the next request starts afresh; one that finished is kept for every other requester.
    async fn run(self: Arc<Self>, store: FsStore, hash: Hash, limit: u32, flight: Arc<Flight>) {
        let outcome = self.build(&store, hash, limit, &flight).await;
        let stopped = flight.stop.load(Ordering::SeqCst);
        let state = match outcome {
            Ok(built) if !stopped => State::Done(built.map(Arc::new)),
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

    /// Reads the blob, cuts it and hashes the chunks, and writes the frame. `Err` is no permit.
    async fn build(
        &self,
        store: &FsStore,
        hash: Hash,
        frame_limit: u32,
        flight: &Arc<Flight>,
    ) -> Result<Option<Built>, ()> {
        let Ok(_permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            return Err(());
        };
        #[cfg(test)]
        self.started.fetch_add(1, Ordering::SeqCst);
        let Some(work) = self.work.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
            return Ok(None);
        };
        let size = match store.blobs().status(hash).await {
            Ok(BlobStatus::Complete { size }) => size,
            _ => return Ok(None),
        };
        let Some(params) = (size >= SMALLEST)
            .then(|| params_for(size, frame_limit))
            .flatten()
        else {
            return Ok(None);
        };
        #[cfg(test)]
        {
            let delay = *self.delay.lock().unwrap_or_else(|p| p.into_inner());
            let until = std::time::Instant::now() + delay;
            while std::time::Instant::now() < until && !flight.stop.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }
        let (tx, rx) = mpsc::channel::<Vec<u8>>(QUEUE);
        let hasher = {
            let flight = Arc::clone(flight);
            tokio::task::spawn_blocking(move || cut(rx, params, &flight.stop))
        };
        read_blob(store, hash, size, tx, &flight.stop).await;
        let Ok(Some((entries, read))) = hasher.await else {
            return Ok(None);
        };
        if read != size || flight.stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let Ok(list) = SigList::new(params.avg(), entries) else {
            return Ok(None);
        };
        let path = work.join(format!("delta-{hash}.sig"));
        let written = async {
            tokio::fs::create_dir_all(&work).await?;
            tokio::fs::write(&path, frame(&list.encode())).await
        }
        .await;
        Ok(written.is_ok().then_some(Built { size, path }))
    }
}

/// Sends the blob to `tx` in the order it lies, stopping when told to or when the other end is gone.
///
/// The blob is read as a bao stream and only its leaves are kept. `export_ranges` would give the same
/// bytes, but in pieces of 1 KiB, one message each, which is a few tens of MiB/s whatever the disk;
/// the bao export reads whole chunk groups of 16 KiB.
async fn read_blob(
    store: &FsStore,
    hash: Hash,
    size: u64,
    tx: mpsc::Sender<Vec<u8>>,
    stop: &AtomicBool,
) {
    use iroh_blobs::{api::blobs::EncodedItem, protocol::ChunkRanges};
    use n0_future::StreamExt;

    let _ = size;
    let mut stream = std::pin::pin!(store.blobs().export_bao(hash, ChunkRanges::all()).stream());
    // Leaves come in order, so pieces are joined into blocks before they are queued: the queue holds
    // a few megabytes, not a few hundred messages.
    let mut block = Vec::with_capacity(super::BLOCK);
    while let Some(item) = stream.next().await {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        match item {
            EncodedItem::Leaf(leaf) => {
                block.extend_from_slice(&leaf.data);
                if block.len() >= super::BLOCK {
                    let full = std::mem::replace(&mut block, Vec::with_capacity(super::BLOCK));
                    if tx.send(full).await.is_err() {
                        return;
                    }
                }
            }
            EncodedItem::Size(_) | EncodedItem::Parent(_) => {}
            EncodedItem::Done => break,
            EncodedItem::Error(_) => return,
        }
    }
    if !block.is_empty() {
        let _ = tx.send(block).await;
    }
}

/// The blocking half: cuts what arrives and hashes each chunk, on a thread that may take its time.
/// `None` is a list that cannot be made: stopped, more entries than a message holds, or a chunk
/// the format has no length for.
fn cut(
    mut rx: mpsc::Receiver<Vec<u8>>,
    params: sluice_delta::Params,
    stop: &AtomicBool,
) -> Option<(Vec<Entry>, u64)> {
    let mut cutter = StreamCut::new(params);
    let mut entries: Vec<Entry> = Vec::new();
    let mut sound = true;
    let mut read = 0u64;
    while let Some(piece) = rx.blocking_recv() {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        read += piece.len() as u64;
        cutter.feed(&piece, |chunk| match Entry::of_bytes(chunk) {
            Ok(entry) => entries.push(entry),
            Err(_) => sound = false,
        });
        if !sound || entries.len() > MAX_ENTRIES as usize {
            return None;
        }
    }
    cutter.finish(|chunk| match Entry::of_bytes(chunk) {
        Ok(entry) => entries.push(entry),
        Err(_) => sound = false,
    });
    (sound && !stop.load(Ordering::SeqCst)).then_some((entries, read))
}
