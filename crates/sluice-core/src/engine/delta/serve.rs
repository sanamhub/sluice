//! The provider's half: signatures for a root it is serving, to an endpoint that has already asked
//! for that root, and the need-ranges that come back.
//!
//! Signatures flow from here to the requester and never the other way. The only thing read from the
//! requester is the root it asks about and one `NeedRanges`, which is checked and then dropped: it
//! changes nothing the provider does.

use std::{
    collections::BTreeSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

use iroh::endpoint::{Connection, RecvStream, SendStream};
#[cfg(test)]
use iroh_blobs::api::proto::BlobStatus;
use iroh_blobs::{Hash, store::fs::FsStore};
use n0_future::time::timeout;
use sluice_delta::NeedRanges;
use tokio::io::AsyncWriteExt;

use super::{MAX_NEED_FRAME, REQUEST_WAIT, list::Lists, read_frame};
use crate::engine::hello::{Hello, MAX_FRAME_BYTES};
use crate::engine::policy::{Asked, Places};

/// What a sender serves signatures from: its store, and the policy state that says who may ask.
pub(crate) struct Delta {
    store: FsStore,
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    asked: Arc<Mutex<Asked>>,
    places: Arc<Mutex<Places>>,
    lists: Arc<Lists>,
    served: AtomicU32,
    /// Bytes a test serves in place of the list, with their own length prefix, so a hostile frame
    /// is reached through a real transfer. A send has no flag for it.
    #[cfg(test)]
    forced: Mutex<Option<Vec<u8>>>,
    /// The need-ranges that passed every check, for a test that reads what a requester sent.
    #[cfg(test)]
    needs: Mutex<Vec<NeedRanges>>,
}

impl std::fmt::Debug for Delta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delta")
            .field("served", &self.served.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Delta {
    /// A sender's delta state over `store`, answering endpoints `asked` records.
    pub(crate) fn new(
        store: FsStore,
        allowed: Arc<Mutex<BTreeSet<Hash>>>,
        asked: Arc<Mutex<Asked>>,
        places: Arc<Mutex<Places>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            allowed,
            asked,
            places,
            lists: Lists::new(),
            served: AtomicU32::new(0),
            #[cfg(test)]
            forced: Mutex::new(None),
            #[cfg(test)]
            needs: Mutex::new(Vec::new()),
        })
    }

    /// How many signature lists this sender has written.
    #[cfg(test)]
    pub(crate) fn served(&self) -> u32 {
        self.served.load(Ordering::SeqCst)
    }

    /// Serves `bytes` in place of the next lists. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn force_frame(&self, bytes: Vec<u8>) {
        *self.forced.lock().unwrap_or_else(|p| p.into_inner()) = Some(bytes);
    }

    /// The need-ranges this sender accepted. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn needs(&self) -> Vec<NeedRanges> {
        self.needs.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Whether `peer` may be told about `hash`: it is content this send serves, the peer asked for
    /// it on a content connection the policy let in, and on a send that counts places it holds one.
    /// The same rule a receipt follows, because the list says as much about the content as a
    /// request for it does.
    fn admitted(&self, peer: &[u8; 32], hash: Hash) -> bool {
        let allowed = self
            .allowed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .contains(&hash);
        let asked = self
            .asked
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .peer_asked(peer, hash);
        let places = self.places.lock().unwrap_or_else(|p| p.into_inner());
        allowed && asked && (places.limit().is_none() || places.holds(peer))
    }

    /// Answers one request on a `sluice/1` stream after both greetings, and finishes the stream. A
    /// refusal writes nothing, so a receiver cannot tell a sender with no list from one that
    /// declined.
    pub(in crate::engine) async fn serve(
        &self,
        conn: &Connection,
        theirs: Hello,
        recv: &mut RecvStream,
        mut send: SendStream,
    ) -> std::io::Result<()> {
        let done = |mut send: SendStream| send.finish().map_err(std::io::Error::other);
        // The stream carries the root and then ends: one byte more than the root is read, and
        // nothing else is taken from the requester on it.
        let read = timeout(REQUEST_WAIT, recv.read_to_end(33)).await;
        let Some(root) = read
            .ok()
            .and_then(Result::ok)
            .and_then(|bytes| super::parse_request(&bytes).ok())
        else {
            return done(send);
        };
        let hash = Hash::from(root);
        let peer = *conn.remote_id().as_bytes();
        if !self.admitted(&peer, hash) {
            return done(send);
        }
        let Some((bytes, size)) = self.list(hash, theirs.max_frame_bytes, conn).await else {
            return done(send);
        };
        send.write_all(&bytes).await?;
        send.flush().await?;
        self.served.fetch_add(1, Ordering::SeqCst);
        self.read_need(conn, root, size).await;
        done(send)
    }

    /// The one `NeedRanges` the requester sends in a stream of its own. It is read and checked so a
    /// peer that sends anything else is noticed, and nothing is done with it.
    async fn read_need(&self, conn: &Connection, root: [u8; 32], size: u64) {
        let Ok(Ok(mut recv)) = timeout(REQUEST_WAIT, conn.accept_uni()).await else {
            return;
        };
        let limit = MAX_NEED_FRAME.min(u64::from(MAX_FRAME_BYTES));
        let Ok(Ok(bytes)) = timeout(REQUEST_WAIT, read_frame(&mut recv, limit)).await else {
            return;
        };
        let accepted = NeedRanges::decode(&bytes)
            .ok()
            .filter(|need| need.root() == &root)
            .filter(|need| need.ranges().last().is_none_or(|last| last.end() <= size));
        #[cfg(test)]
        if let Some(need) = accepted {
            self.needs
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(need);
        }
        #[cfg(not(test))]
        let _ = accepted;
    }

    /// The frame that carries the list for `hash`, and the size of the content it is about, or
    /// `None` when there is nothing to say. The list is built once for every requester, and the
    /// build stops when the last requester waiting for it has gone.
    async fn list(
        &self,
        hash: Hash,
        frame_limit: u32,
        conn: &Connection,
    ) -> Option<(Vec<u8>, u64)> {
        #[cfg(test)]
        let forced = self
            .forced
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        #[cfg(test)]
        if let Some(bytes) = forced {
            let size = match self.store.blobs().status(hash).await {
                Ok(BlobStatus::Complete { size }) => size,
                _ => return None,
            };
            return Some((bytes, size));
        }
        self.lists.get(&self.store, hash, frame_limit, conn).await
    }

    /// Where the lists are written: a folder that goes with the send.
    pub(crate) fn set_work(&self, dir: std::path::PathBuf) {
        self.lists.set_work(dir);
    }

    /// Makes each list build wait `by` before it reads. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn delay_builds(&self, by: std::time::Duration) {
        self.lists.delay_builds(by);
    }

    /// How many list builds have started. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn builds(&self) -> usize {
        self.lists.builds()
    }

    /// How many list builds could start now. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn free_permits(&self) -> usize {
        self.lists.free_permits()
    }
}
