//! The one access policy of a provider. It answers `get`, `get_many` and `observe` with the same
//! set of allowed hashes, refuses every push, and refuses everything after the ticket expiry.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use iroh_blobs::{Hash, provider::events::EventSender, store::fs::FsStore};
use n0_future::task::AbortOnDropHandle;
use tokio::sync::mpsc::UnboundedSender;

mod asked;
mod delivery;
mod gate;
mod places;

use asked::Asked;
use gate::{Gate, mask};
#[cfg(test)]
pub(crate) use places::Admission;
pub(crate) use places::Places;

use super::endpoint::Receiver;

/// The provider's policy and the task that answers its request events.
pub struct Policy {
    /// Handed to `BlobsProtocol`, which uses it for every request.
    pub events: EventSender,
    /// Payload served per connection and blob, so a test can see what a request moved.
    #[cfg(test)]
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    /// What each connection asked for, so the provider can check a completion code against it.
    asked: Arc<Mutex<Asked>>,
    /// Aborted when the provider goes away, so no event task outlives it.
    pub _task: AbortOnDropHandle<()>,
}

impl Policy {
    /// Payload this provider sent of `hash`, over every connection, since it started.
    #[cfg(test)]
    pub fn served(&self, hash: Hash) -> u64 {
        lock(&self.totals)
            .iter()
            .filter(|((_, served), _)| *served == hash)
            .map(|(_, bytes)| *bytes)
            .sum()
    }

    /// The roots `connection` asked for and was allowed to read.
    pub fn asked_by(&self, connection: u64) -> BTreeSet<Hash> {
        lock(&self.asked).of(connection)
    }
}

impl std::fmt::Debug for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Policy").finish_non_exhaustive()
    }
}

/// What the provider reports back so a sender can stop after one download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeEvent {
    /// Every byte of this root has now been served, across every connection that asked for it.
    Downloaded {
        /// The blob the receiver asked for.
        hash: Hash,
        /// The connection the request that finished it arrived on.
        connection: u64,
    },
    /// That connection closed. A root is not delivered because a connection that asked for part of
    /// it went away: a receive that was killed must be able to come back for the rest.
    Closed {
        /// The connection that closed.
        connection: u64,
    },
}

/// [`start`] with an allowlist the caller keeps. A live send adds the root of each segment it
/// seals, and the same set is what the policy answers from.
#[allow(clippy::too_many_arguments)]
pub fn start_shared(
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    rate: Option<Arc<super::Rate>>,
    ids: Arc<super::conn::ConnIds>,
    places: Arc<Mutex<Places>>,
    receivers: UnboundedSender<Receiver>,
) -> Policy {
    let (events, mut rx) = EventSender::channel(32, mask(rate.is_some()));
    let totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>> = Arc::default();
    let asked: Arc<Mutex<Asked>> = Arc::default();
    let gate = Gate::new(
        allowed,
        expiry,
        done,
        store,
        Arc::clone(&totals),
        Arc::default(),
        Arc::clone(&asked),
        Arc::clone(&ids),
        rate,
        Arc::clone(&places),
        receivers,
    );
    let task = AbortOnDropHandle::new(n0_future::task::spawn(async move {
        while let Some(message) = rx.recv().await {
            gate.answer(message).await;
        }
    }));
    Policy {
        events,
        #[cfg(test)]
        totals,
        asked,
        _task: task,
    }
}

/// The lock cannot be held across an await and every holder only edits a map, so a panic in one
/// task poisoning it changes nothing here.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
