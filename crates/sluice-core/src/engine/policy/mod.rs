//! The one access policy of a provider. It answers `get`, `get_many` and `observe` with the same
//! set of allowed hashes, refuses every push, and refuses everything after the ticket expiry.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use iroh_blobs::{
    Hash,
    api::proto::BlobStatus,
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, RequestUpdate,
    },
    store::fs::FsStore,
};
use n0_future::task::AbortOnDropHandle;

mod delivery;
use delivery::{add, covers, credited, requested};
use tokio::sync::mpsc::UnboundedSender;

use super::conn::ConnIds;

/// The byte ranges of one root this process has served, merged and in ascending order. A range
/// asked for twice counts once, so two requests for the first half are not the whole file.
type Served = BTreeMap<Hash, Vec<(u64, u64)>>;

/// The roots each connection asked for and was allowed to read. The completion code is believed
/// only from a connection whose set holds every root of the transfer.
#[derive(Debug, Default)]
struct Asked {
    /// Keyed by the id [`ConnIds`] gave the connection, which only grows, so the lowest key is the
    /// connection that asked first.
    by: BTreeMap<u64, BTreeSet<Hash>>,
}

/// How many connections the provider remembers. Past this the oldest record goes, so a peer that
/// opens thousands of connections can only make the sender serve longer, never shorter.
const MAX_ASKED: usize = 1024;

impl Asked {
    /// Notes that `connection` asked for `hash`. A full map loses the record of the connection
    /// that asked first: ids grow with time, so that is the oldest insertion and not merely the
    /// lowest number a peer happened to land on.
    fn remember(&mut self, connection: u64, hash: Hash) {
        if self.by.len() >= MAX_ASKED
            && !self.by.contains_key(&connection)
            && let Some(oldest) = self.by.keys().next().copied()
        {
            self.by.remove(&oldest);
        }
        self.by.entry(connection).or_default().insert(hash);
    }

    /// The roots `connection` asked for and was allowed to read.
    fn of(&self, connection: u64) -> BTreeSet<Hash> {
        self.by.get(&connection).cloned().unwrap_or_default()
    }
}

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

/// What every request is answered against: the allowlist, the ticket deadline, the reporter, and
/// the maps that say how much of each blob this process has served.
struct Gate {
    /// Shared with a live send, which adds each segment as it seals it.
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    served: Arc<Mutex<Served>>,
    asked: Arc<Mutex<Asked>>,
    ids: Arc<ConnIds>,
    rate: Option<Arc<super::Rate>>,
}

/// [`start`] with an allowlist the caller keeps. A live send adds the root of each segment it
/// seals, and the same set is what the policy answers from.
pub fn start_shared(
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    rate: Option<Arc<super::Rate>>,
    ids: Arc<ConnIds>,
) -> Policy {
    let (events, mut rx) = EventSender::channel(32, mask(rate.is_some()));
    let totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>> = Arc::default();
    let asked: Arc<Mutex<Asked>> = Arc::default();
    let gate = Gate {
        allowed,
        expiry,
        done,
        store,
        totals: Arc::clone(&totals),
        served: Arc::default(),
        asked: Arc::clone(&asked),
        ids: Arc::clone(&ids),
        rate,
    };
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

fn mask(limited: bool) -> EventMask {
    EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::InterceptLog,
        get_many: RequestMode::Intercept,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
        // Each chunk is paced here only when a cap was asked for: otherwise every 16 KiB chunk
        // would pay for a round trip to this handler for nothing.
        throttle: if limited {
            iroh_blobs::provider::events::ThrottleMode::Intercept
        } else {
            iroh_blobs::provider::events::ThrottleMode::None
        },
    }
}

fn answer(ok: bool) -> Result<(), AbortReason> {
    if ok {
        Ok(())
    } else {
        Err(AbortReason::Permission)
    }
}

impl Gate {
    /// Answers one request. A get is checked against the allowlist, then its events decide when
    /// every byte of the blob went out.
    async fn answer(&self, message: ProviderMessage) {
        match message {
            ProviderMessage::GetRequestReceived(mut msg) => {
                let hash = msg.request.hash;
                let ok = self.may(hash);
                // The id this process gave the connection. A connection the handshake never saw
                // has none, so its request is still checked against the allowlist but counted for
                // no completion: an id inherited from a closed connection would let a peer end a
                // send it never received.
                let connection = self.ids.own(msg.connection_id).unwrap_or_default();
                let whole = msg.request.ranges.is_all();
                let asked = requested(&msg.request.ranges);
                if msg.tx.send(answer(ok)).await.is_err() || !ok {
                    return;
                }
                if connection != 0 {
                    lock(&self.asked).remember(connection, hash);
                }
                let done = self.done.clone();
                let store = self.store.clone();
                let totals = Arc::clone(&self.totals);
                let served = Arc::clone(&self.served);
                n0_future::task::spawn(async move {
                    let sent = Sent {
                        store: &store,
                        totals: &totals,
                        served: &served,
                        connection,
                    };
                    while let Ok(Some(update)) = msg.rx.recv().await {
                        let Some(covered) = sent.checked(hash, &asked, update, whole).await else {
                            continue;
                        };
                        // Id 0 is no connection of this process, so a delivery on it is not one a
                        // completion code can be matched to.
                        if covered && connection != 0 {
                            let _ = done.send(ServeEvent::Downloaded { hash, connection });
                        }
                        break;
                    }
                });
            }
            ProviderMessage::GetManyRequestReceived(msg) => {
                let ok = self.may_all(&msg.request.hashes);
                msg.tx.send(answer(ok)).await.ok();
            }
            ProviderMessage::ObserveRequestReceived(msg) => {
                let ok = self.may(msg.request.hash);
                msg.tx.send(answer(ok)).await.ok();
            }
            ProviderMessage::ConnectionClosed(msg) => {
                if let Some(connection) = self.ids.own(msg.connection_id) {
                    let _ = self.done.send(ServeEvent::Closed { connection });
                }
            }
            ProviderMessage::Throttle(msg) => {
                // The wait is a task of its own, not this gate's: one paced chunk would otherwise
                // hold up the permission checks and close events of every other connection. The
                // provider waits for the answer before it writes, so the pacing still applies.
                let rate = self.rate.clone();
                n0_future::task::spawn(async move {
                    if let Some(rate) = rate {
                        rate.acquire(msg.size).await;
                    }
                    let _ = msg.tx.send(Ok(())).await;
                });
            }
            _ => {}
        }
    }

    /// The one access rule: an allowed hash, before the ticket's deadline.
    fn may(&self, hash: Hash) -> bool {
        !expired(self.expiry) && lock(&self.allowed).contains(&hash)
    }

    /// The same rule for a request that names several hashes at once: every one has to be allowed,
    /// so one refused hash refuses the whole request.
    fn may_all(&self, hashes: &[Hash]) -> bool {
        if expired(self.expiry) {
            return false;
        }
        let allowed = lock(&self.allowed);
        hashes.iter().all(|hash| allowed.contains(hash))
    }
}

/// One connection's view of what this process has served, so the two maps travel together.
struct Sent<'a> {
    store: &'a FsStore,
    totals: &'a Mutex<BTreeMap<(u64, Hash), u64>>,
    served: &'a Mutex<Served>,
    connection: u64,
}

impl Sent<'_> {
    /// The delivery this update decides, or `None` for one that changes nothing yet. A completed
    /// request sent everything it asked for, and an aborted one sent the bytes it reports from the
    /// front of what it asked for, which is the order a provider writes in.
    async fn checked(
        &self,
        hash: Hash,
        asked: &[(u64, u64)],
        update: RequestUpdate,
        whole: bool,
    ) -> Option<bool> {
        let (stats, complete) = match update {
            RequestUpdate::Completed(stats) => (stats.stats, true),
            RequestUpdate::Aborted(stats) => (stats.stats, false),
            _ => return None,
        };
        let sent_bytes = stats.payload_bytes_sent;
        let size = match self.store.blobs().status(hash).await {
            Ok(BlobStatus::Complete { size }) => size,
            _ => return Some(false),
        };
        {
            let mut totals = lock(self.totals);
            let sent = totals.entry((self.connection, hash)).or_default();
            *sent = sent.saturating_add(sent_bytes);
        }
        // A blob with no bytes has no range to cover, so a completed request for it is the blob.
        if size == 0 {
            return Some(complete);
        }
        let asked = credited(asked, whole, size, complete, sent_bytes);
        let mut served = lock(self.served);
        let spans = served.entry(hash).or_default();
        for (start, end) in asked {
            add(spans, start, end);
        }
        Some(covers(spans, size))
    }
}

/// The lock cannot be held across an await and every holder only edits a map, so a panic in one
/// task poisoning it changes nothing here.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn expired(expiry: Option<u64>) -> bool {
    expired_at(expiry, super::now_ms())
}

/// The ticket boundary, with the clock as an argument so the rule is checked without waiting. A
/// request at `expiry - 1` is served and one at `expiry` is not, the same boundary as a JWT.
fn expired_at(expiry: Option<u64>, now: u64) -> bool {
    expiry.is_some_and(|expiry| now >= expiry)
}

#[cfg(test)]
mod tests {
    use super::{Asked, MAX_ASKED, expired_at};
    use crate::engine::conn::ConnIds;
    use iroh_blobs::Hash;

    #[test]
    fn the_boundary_is_the_ticket_boundary() {
        assert!(!expired_at(None, u64::MAX));
        assert!(expired_at(Some(0), 0));
        assert!(!expired_at(Some(10), 9));
        assert!(expired_at(Some(10), 10));
        assert!(expired_at(Some(10), 11));
    }

    /// A peer with no ticket that lands on a closed connection's iroh id must not inherit the
    /// record of the connection it replaced: it would otherwise have its completion code believed
    /// for roots it never asked for.
    #[test]
    fn a_connection_that_reuses_an_id_inherits_nothing() {
        let ids = ConnIds::default();
        let mut asked = Asked::default();
        let root = Hash::from([1u8; 32]);
        let first = ids.assign(9);
        asked.remember(first, root);
        assert!(asked.of(first).contains(&root));
        ids.closed(9, first);
        let second = ids.assign(9);
        assert_ne!(first, second);
        assert!(asked.of(second).is_empty());
    }

    /// The cap drops the connection that asked first. Ids grow with time, so that is the lowest
    /// key and not whichever id a peer happened to land on.
    #[test]
    fn the_cap_evicts_the_oldest_connection() {
        let mut asked = Asked::default();
        let root = Hash::from([2u8; 32]);
        for connection in 1..=MAX_ASKED as u64 {
            asked.remember(connection, root);
        }
        assert_eq!(asked.by.len(), MAX_ASKED);
        asked.remember(MAX_ASKED as u64 + 1, root);
        assert_eq!(asked.by.len(), MAX_ASKED);
        assert!(asked.of(1).is_empty(), "the first connection was kept");
        assert!(!asked.of(MAX_ASKED as u64 + 1).is_empty());
        assert!(!asked.of(2).is_empty());
    }
}
