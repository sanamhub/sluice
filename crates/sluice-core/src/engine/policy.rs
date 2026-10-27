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
use tokio::sync::mpsc::UnboundedSender;

/// The provider's policy and the task that answers its request events.
pub struct Policy {
    /// Handed to `BlobsProtocol`, which uses it for every request.
    pub events: EventSender,
    /// The cap the command set, if any. A send sets it after it printed its ticket, so the handle
    /// is shared with the event task rather than fixed when the provider starts.
    rate: Arc<Mutex<Option<Arc<super::Rate>>>>,
    /// Payload served per connection and blob, so a test can see what a request moved.
    #[cfg(test)]
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    /// Aborted when the provider goes away, so no event task outlives it.
    pub _task: AbortOnDropHandle<()>,
}

impl Policy {
    /// Caps the bytes this provider sends per second, shared by every request.
    pub fn set_rate(&self, rate: Option<Arc<super::Rate>>) {
        *lock(&self.rate) = rate;
    }

    /// Payload this provider sent of `hash`, over every connection, since it started.
    #[cfg(test)]
    pub fn served(&self, hash: Hash) -> u64 {
        lock(&self.totals)
            .iter()
            .filter(|((_, served), _)| *served == hash)
            .map(|(_, bytes)| *bytes)
            .sum()
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
/// the maps that say how much of each blob every connection has served.
/// The cap this command set, paced on one schedule shared by every request. A cell rather than a
/// value, because a send sets it after the provider has started.
type RateCell = Arc<Mutex<Option<Arc<super::Rate>>>>;

struct Gate {
    /// Shared with a live send, which adds each segment as it seals it.
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    delivered: Arc<Mutex<BTreeMap<Hash, u64>>>,
    rate: RateCell,
}

/// [`start`] with an allowlist the caller keeps. A live send adds the root of each segment it
/// seals, and the same set is what the policy answers from.
pub fn start_shared(
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
) -> Policy {
    let (events, mut rx) = EventSender::channel(32, mask());
    let totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>> = Arc::default();
    let rate: RateCell = Arc::default();
    let gate = Gate {
        allowed,
        expiry,
        done,
        store,
        totals: Arc::clone(&totals),
        delivered: Arc::default(),
        rate: Arc::clone(&rate),
    };
    let task = AbortOnDropHandle::new(n0_future::task::spawn(async move {
        while let Some(message) = rx.recv().await {
            gate.answer(message).await;
        }
    }));
    Policy {
        events,
        rate,
        #[cfg(test)]
        totals,
        _task: task,
    }
}

fn mask() -> EventMask {
    EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::InterceptLog,
        get_many: RequestMode::Intercept,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
        // Every chunk can be paced here. Without a cap the answer is immediate, and the handler
        // is the only place that sees each chunk before it goes out.
        throttle: iroh_blobs::provider::events::ThrottleMode::Intercept,
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
    async fn answer(&self, message: ProviderMessage) {
        match message {
            ProviderMessage::GetRequestReceived(mut msg) => {
                let hash = msg.request.hash;
                // Any range of an allowed hash is served, including a hash sequence request for
                // the manifest root. The allowed set is the access rule, not the format.
                let ok = self.may(hash);
                let connection = msg.connection_id;
                let whole = msg.request.ranges.is_all();
                if msg.tx.send(answer(ok)).await.is_err() || !ok {
                    return;
                }
                let done = self.done.clone();
                let store = self.store.clone();
                let totals = Arc::clone(&self.totals);
                let delivered = Arc::clone(&self.delivered);
                n0_future::task::spawn(async move {
                    while let Ok(Some(update)) = msg.rx.recv().await {
                        match update {
                            RequestUpdate::Completed(stats) => {
                                let complete = covered(
                                    &store,
                                    hash,
                                    stats.stats.payload_bytes_sent,
                                    &totals,
                                    &delivered,
                                    connection,
                                )
                                .await;
                                if whole || complete {
                                    let _ = done.send(ServeEvent::Downloaded { hash, connection });
                                }
                                break;
                            }
                            RequestUpdate::Aborted(_) => break,
                            _ => {}
                        }
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
                let _ = self.done.send(ServeEvent::Closed {
                    connection: msg.connection_id,
                });
            }
            // The provider asks before each chunk goes out. Answering now would send it now, so
            // the wait for this chunk's turn is the whole rate cap.
            ProviderMessage::Throttle(msg) => {
                let rate = lock(&self.rate).clone();
                if let Some(rate) = rate {
                    rate.acquire(msg.size).await;
                }
                msg.tx.send(Ok(())).await.ok();
            }
            // Push is disabled in the mask, so a push is refused before this task sees it.
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

/// Whether the payload served for `hash` has reached the whole blob. A chunk group request for a
/// small file covers all of it; a request for the last block of a large one does not. The count is
/// per root and not per connection, so a receive that fetches half a file, is killed and fetches
/// the other half from a new connection still finishes the root. Asking twice for the same range
/// could reach the size without the middle; the sender only stops serving, and a client that wants
/// to keep it alive asks for what it needs.
async fn covered(
    store: &FsStore,
    hash: Hash,
    payload: u64,
    totals: &Mutex<BTreeMap<(u64, Hash), u64>>,
    delivered: &Mutex<BTreeMap<Hash, u64>>,
    connection: u64,
) -> bool {
    let size = match store.blobs().status(hash).await {
        Ok(BlobStatus::Complete { size }) => size,
        _ => return false,
    };
    let mut totals = lock(totals);
    let sent = totals.entry((connection, hash)).or_default();
    *sent = sent.saturating_add(payload);
    drop(totals);
    let mut delivered = lock(delivered);
    let total = delivered.entry(hash).or_default();
    *total = total.saturating_add(payload);
    *total >= size
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
    use super::expired_at;

    #[test]
    fn the_boundary_is_the_ticket_boundary() {
        assert!(!expired_at(None, u64::MAX));
        assert!(expired_at(Some(0), 0));
        assert!(!expired_at(Some(10), 9));
        assert!(expired_at(Some(10), 10));
        assert!(expired_at(Some(10), 11));
    }
}
