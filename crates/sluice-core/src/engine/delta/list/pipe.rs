//! The two halves of a build: the task that reads a blob to a channel, and the blocking thread that
//! cuts and hashes what arrives.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use iroh_blobs::{Hash, store::fs::FsStore};
use sluice_delta::{Entry, MAX_ENTRIES, StreamCut};
use tokio::sync::mpsc;

use super::Flight;

/// Reads a blob to a channel, in the order it lies.
pub(super) struct Reader<'a> {
    pub(super) store: &'a FsStore,
    pub(super) hash: Hash,
    pub(super) flight: &'a Arc<Flight>,
    /// What a test makes the reader wait for each block.
    pub(super) delay: std::time::Duration,
}

impl Reader<'_> {
    /// Sends the blob to `tx`, stopping when told to or when the other end is gone. `true` is the
    /// whole blob sent.
    ///
    /// The blob is read as a bao stream and only its leaves are kept. `export_ranges` would give
    /// the same bytes, but in pieces of 1 KiB, one message each, which is a few tens of MiB/s
    /// whatever the disk; the bao export reads whole chunk groups of 16 KiB.
    pub(super) async fn run(&self, tx: mpsc::Sender<Vec<u8>>) -> bool {
        use iroh_blobs::{api::blobs::EncodedItem, protocol::ChunkRanges};
        use n0_future::StreamExt;

        let stop = &self.flight.stop;
        let mut stream = std::pin::pin!(
            self.store
                .blobs()
                .export_bao(self.hash, ChunkRanges::all())
                .stream()
        );
        // Leaves come in order, so pieces are joined into blocks before they are queued: the queue
        // holds a few megabytes, not a few hundred messages.
        let mut block = Vec::with_capacity(crate::engine::delta::BLOCK);
        while let Some(item) = stream.next().await {
            if stop.load(Ordering::SeqCst) {
                return false;
            }
            match item {
                EncodedItem::Leaf(leaf) => {
                    block.extend_from_slice(&leaf.data);
                    if block.len() >= crate::engine::delta::BLOCK {
                        let full = std::mem::replace(
                            &mut block,
                            Vec::with_capacity(crate::engine::delta::BLOCK),
                        );
                        self.flight
                            .read
                            .fetch_add(full.len() as u64, Ordering::SeqCst);
                        if tx.send(full).await.is_err() {
                            return false;
                        }
                        if !self.delay.is_zero() {
                            tokio::time::sleep(self.delay).await;
                        }
                    }
                }
                EncodedItem::Size(_) | EncodedItem::Parent(_) => {}
                EncodedItem::Done => break,
                EncodedItem::Error(_) => return false,
            }
        }
        if !block.is_empty() {
            self.flight
                .read
                .fetch_add(block.len() as u64, Ordering::SeqCst);
            return tx.send(block).await.is_ok();
        }
        true
    }
}

/// The blocking half: cuts what arrives and hashes each chunk, on a thread that may take its time.
/// `None` is a list that cannot be made: stopped, more entries than a message holds, or a chunk
/// the format has no length for. `delay` is what a test makes it wait for each piece.
pub(super) fn cut(
    mut rx: mpsc::Receiver<Vec<u8>>,
    params: sluice_delta::Params,
    stop: &AtomicBool,
    delay: std::time::Duration,
) -> Option<(Vec<Entry>, u64)> {
    let mut cutter = StreamCut::new(params);
    let mut entries: Vec<Entry> = Vec::new();
    let mut sound = true;
    let mut read = 0u64;
    while let Some(piece) = rx.blocking_recv() {
        if stop.load(Ordering::SeqCst) {
            return None;
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
            if stop.load(Ordering::SeqCst) {
                return None;
            }
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
