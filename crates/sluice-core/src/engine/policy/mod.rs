//! The one access policy of a provider. It answers `get`, `get_many` and `observe` with the same
//! set of allowed hashes, refuses every push, and refuses everything after the ticket expiry.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use iroh_blobs::{
    Hash,
    provider::events::{ConnectMode, EventSender, ObserveMode, RequestMode, ThrottleMode},
    store::fs::FsStore,
};
use n0_future::task::AbortOnDropHandle;
use tokio::sync::mpsc::UnboundedSender;

mod asked;
mod delivery;
mod gate;
mod places;

use super::endpoint::Receiver;
pub(crate) use asked::Asked;
use gate::{Gate, mask};
#[cfg(test)]
pub(crate) use places::Admission;
#[cfg(test)]
pub(crate) use places::REFUSAL_LINES;
pub(crate) use places::{Lines, Places};

/// The provider's policy and the task that answers its request events.
pub struct Policy {
    /// Handed to `BlobsProtocol`, which uses it for every request.
    pub events: EventSender,
    /// Payload served per connection and blob, so a test can see what a request moved.
    #[cfg(test)]
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    /// How many requests are still being accounted for, so a test that reads [`Policy::served`]
    /// waits for that count instead of racing the task that writes it. A request's updates are
    /// read by a task of its own, which runs after the receiver already holds the bytes.
    #[cfg(test)]
    accounting: Arc<Accounting>,
    /// What each connection asked for, so the provider can check a completion code against it.
    asked: Arc<Mutex<Asked>>,
    /// Aborted when the provider goes away, so no event task outlives it.
    pub _task: AbortOnDropHandle<()>,
}

/// The requests whose byte count a task has not finished writing, and a test-only wait for them.
/// An update is queued before a receiver can hold the last byte, so the wait ends as soon as the
/// task is given a turn.
#[derive(Debug, Default)]
pub(super) struct Accounting {
    pending: std::sync::atomic::AtomicUsize,
}

impl Accounting {
    /// One more request whose events a task will read.
    pub(super) fn begin(&self) {
        self.pending
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// That request's task has read its last update and written what it counted.
    pub(super) fn end(&self) {
        self.pending
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// Waits, briefly, until every request's count has been written. Bounded, because a request
    /// that ends without an update has its channel closed instead, and a broken test should fail
    /// on its own assertion rather than hang.
    #[cfg(test)]
    fn wait(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.pending.load(std::sync::atomic::Ordering::SeqCst) > 0 {
            if std::time::Instant::now() >= deadline {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

impl Policy {
    /// Payload this provider sent of `hash`, over every connection, since it started. Waits for a
    /// request whose updates are still being counted, so a test that has just received every byte
    /// sees them.
    #[cfg(test)]
    pub fn served(&self, hash: Hash) -> u64 {
        self.accounting.wait();
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

    /// The record of what every connection asked for, so the receipt handler can tell whether an
    /// endpoint asked for the root it is asking a receipt for.
    pub(crate) fn asked_handle(&self) -> Arc<Mutex<Asked>> {
        Arc::clone(&self.asked)
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
    let accounting: Arc<Accounting> = Arc::default();
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
        Arc::clone(&accounting),
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
        #[cfg(test)]
        accounting,
        asked,
        _task: task,
    }
}

/// The event mask a provider registers, event by event: the mode of a send with no rate limit, and
/// the mode of one with a limit. The `access/` vectors are this list, and the conformance runner
/// compares them with the mask [`start_shared`] really registers, so a change here cannot leave
/// `SPEC.md` section 5 behind.
pub fn access_modes() -> [(&'static str, &'static str, &'static str); 6] {
    let plain = mask(false);
    let limited = mask(true);
    [
        (
            "connected",
            connect_mode(plain.connected),
            connect_mode(limited.connected),
        ),
        ("get", request_mode(plain.get), request_mode(limited.get)),
        (
            "get_many",
            request_mode(plain.get_many),
            request_mode(limited.get_many),
        ),
        (
            "observe",
            observe_mode(plain.observe),
            observe_mode(limited.observe),
        ),
        ("push", request_mode(plain.push), request_mode(limited.push)),
        (
            "throttle",
            throttle_mode(plain.throttle),
            throttle_mode(limited.throttle),
        ),
    ]
}

fn connect_mode(mode: ConnectMode) -> &'static str {
    match mode {
        ConnectMode::None => "none",
        ConnectMode::Notify => "notify",
        ConnectMode::Intercept => "intercept",
    }
}

fn request_mode(mode: RequestMode) -> &'static str {
    match mode {
        RequestMode::None => "none",
        RequestMode::Notify => "notify",
        RequestMode::Intercept => "intercept",
        RequestMode::NotifyLog => "notify_log",
        RequestMode::InterceptLog => "intercept_log",
        RequestMode::Disabled => "disabled",
    }
}

fn observe_mode(mode: ObserveMode) -> &'static str {
    match mode {
        ObserveMode::None => "none",
        ObserveMode::Notify => "notify",
        ObserveMode::Intercept => "intercept",
        ObserveMode::Disabled => "disabled",
    }
}

fn throttle_mode(mode: ThrottleMode) -> &'static str {
    match mode {
        ThrottleMode::None => "none",
        ThrottleMode::Intercept => "intercept",
    }
}

/// The lock cannot be held across an await and every holder only edits a map, so a panic in one
/// task poisoning it changes nothing here.
pub(super) fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
