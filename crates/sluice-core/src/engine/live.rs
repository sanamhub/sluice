//! Live content: a file that is still growing, sealed into signed segments and served as a chain
//! of records over `sluice/1`. The records are checked by `sluice_live`; this module only carries
//! them, keeps the roots they name in the provider's allowlist, and writes the segment bytes the
//! receiver asks for from the store.

mod follow;
mod recv;

pub use follow::{CLOSE_AFTER, prepare_live, prepare_live_with_rate};
pub use recv::receive_live;

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex, MutexGuard},
};

use iroh::endpoint::{RecvStream, SendStream};
use iroh_blobs::Hash;
use sluice_live::{Frame, LogClose, LogOpen};
use tokio::{
    io::AsyncWriteExt,
    sync::{broadcast, watch},
};

use super::hello;
use crate::Error;

/// The longest record either side may write: the 144 bytes of a `LogClose`.
const MAX_RECORD: usize = 144;

/// One live stream as the provider serves it: the signed opening record, every frame sealed so
/// far, the close once the file stops growing, and the roots the static policy answers for.
pub struct LiveLog {
    open: LogOpen,
    frames: Mutex<Vec<Frame>>,
    close: Mutex<Option<LogClose>>,
    signals: broadcast::Sender<()>,
    closed: watch::Sender<bool>,
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
}

/// The log and the task that follows the file, held by a send so both end with it.
pub struct LiveSend {
    /// The stream the provider serves.
    pub log: Arc<LiveLog>,
    task: tokio::task::JoinHandle<()>,
}

impl LiveSend {
    pub(super) fn new(log: Arc<LiveLog>, task: tokio::task::JoinHandle<()>) -> Self {
        Self { log, task }
    }

    /// Stops following the file. The provider is closed separately.
    pub fn stop(&self) {
        self.task.abort();
    }
}

impl std::fmt::Debug for LiveLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveLog").finish_non_exhaustive()
    }
}

impl LiveLog {
    pub(super) fn new(open: LogOpen, allowed: Arc<Mutex<BTreeSet<Hash>>>) -> Self {
        let (signals, _) = broadcast::channel(16);
        let (closed, _) = watch::channel(false);
        Self {
            open,
            frames: Mutex::new(Vec::new()),
            close: Mutex::new(None),
            signals,
            closed,
            allowed,
        }
    }

    /// The allowlist this log adds a root to as each segment is sealed.
    pub(super) fn allowed(&self) -> Arc<Mutex<BTreeSet<Hash>>> {
        Arc::clone(&self.allowed)
    }

    /// Adds a sealed frame and allows its segment root. The frame is pushed before the root, so a
    /// receiver that sees the frame can always fetch the bytes it names.
    pub(super) fn push(&self, frame: Frame) {
        lock(&self.frames).push(frame);
        lock(&self.allowed).insert(Hash::from(frame.entry.segment_root));
        let _ = self.signals.send(());
    }

    /// Ends the stream. Every reader that is still writing records wakes up and writes the close.
    pub(super) fn finish(&self, close: LogClose) {
        *lock(&self.close) = Some(close);
        let _ = self.closed.send(true);
        let _ = self.signals.send(());
    }

    /// The roots of every segment, in chain order.
    pub(super) fn roots(&self) -> Vec<[u8; 32]> {
        lock(&self.frames)
            .iter()
            .map(|frame| frame.entry.segment_root)
            .collect()
    }

    /// Damages the signature of one sealed frame, so a receiver meets bytes no sender signed.
    #[cfg(test)]
    pub(super) fn corrupt(&self, at: usize) {
        lock(&self.frames)[at].sig[0] ^= 1;
    }

    /// Puts two sealed frames in the wrong order on the stream.
    #[cfg(test)]
    pub(super) fn swap(&self, first: usize, second: usize) {
        lock(&self.frames).swap(first, second);
    }

    /// Waits until the file stopped growing and its close was signed.
    pub(super) async fn wait_closed(&self) {
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return;
        }
        let _ = closed.wait_for(|closed| *closed).await;
    }

    /// Writes the opening record, every sealed frame and the close, in that order. A reader that
    /// joins late gets the chain from the first segment, as section 6 requires.
    pub(super) async fn write_to(&self, send: &mut SendStream) -> Result<(), Error> {
        write_record(send, &self.open.encode()).await?;
        let mut events = self.signals.subscribe();
        let mut sent = 0;
        loop {
            let (batch, close) = {
                let frames = lock(&self.frames);
                let batch = frames[sent..].to_vec();
                sent = frames.len();
                (batch, *lock(&self.close))
            };
            for frame in batch {
                write_record(send, &frame.encode()).await?;
            }
            if let Some(close) = close {
                return write_record(send, &close.encode()).await;
            }
            // A frame pushed between the copy above and this wait has left a signal behind, which
            // is received at once; nothing sealed is ever missed.
            if events.recv().await.is_err() {
                return Ok(());
            }
        }
    }
}

/// Writes one record as its length and its bytes.
async fn write_record(send: &mut SendStream, record: &[u8]) -> Result<(), Error> {
    send.write_all(&hello::varint_bytes(record.len() as u64))
        .await
        .map_err(|e| Error::Network(format!("the live stream stopped: {e}")))?;
    send.write_all(record)
        .await
        .map_err(|e| Error::Network(format!("the live stream stopped: {e}")))?;
    send.flush()
        .await
        .map_err(|e| Error::Network(format!("the live stream stopped: {e}")))
}

/// Reads one record. The length is a varint and no record is longer than a `LogClose`, so a
/// provider cannot make the receiver set aside more than 144 bytes for one.
pub(super) async fn read_record(recv: &mut RecvStream) -> Result<Vec<u8>, Error> {
    let len = hello::read_stream_varint(recv).await.map_err(stream)?;
    let len = usize::try_from(len).map_err(|_| {
        Error::Verify("a live record's length does not fit this machine".to_string())
    })?;
    if len > MAX_RECORD {
        return Err(Error::Verify(
            "a live record is longer than any live record".to_string(),
        ));
    }
    let mut bytes = vec![0u8; len];
    recv.read_exact(&mut bytes)
        .await
        .map_err(|e| Error::Network(format!("the live stream stopped: {e}")))?;
    Ok(bytes)
}

fn stream(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::InvalidData {
        Error::Verify(format!("the live stream is not readable: {error}"))
    } else {
        Error::Network(format!("the live stream stopped: {error}"))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
