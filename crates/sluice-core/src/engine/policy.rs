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
    /// Payload served per connection and blob, so a test can see what a request moved.
    #[cfg_attr(not(test), allow(dead_code))]
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
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
}

impl std::fmt::Debug for Policy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Policy").finish_non_exhaustive()
    }
}

/// What the provider reports back so a sender can stop after one download.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeEvent {
    /// A request for this hash was completed and, with what this connection asked for before, it
    /// covered the whole blob.
    Downloaded {
        /// The blob the receiver asked for.
        hash: Hash,
        /// The connection the request arrived on.
        connection: u64,
    },
    /// That connection closed, with the set of allowed hashes it fetched anything of. A client
    /// that fetched from every root and then left has nothing more to receive.
    Closed {
        /// The connection that closed.
        connection: u64,
        /// The roots it completed at least one request for.
        served: BTreeSet<Hash>,
    },
}

/// What every request is answered against: the allowlist, the ticket deadline, the reporter, and
/// the two maps that say what was served on which connection.
struct Gate {
    allowed: Arc<BTreeSet<Hash>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    touched: Arc<Mutex<BTreeMap<u64, BTreeSet<Hash>>>>,
}

/// Starts the policy. `done` receives every completed get and every closed connection, so a sender
/// can stop after one download and not cut the last bytes off. `expiry` is UTC milliseconds; at or
/// after it every request is refused. `store` is the provider's own store, which knows each blob's
/// size, so a request that asks for the last block first is not mistaken for the whole file.
pub fn start(
    allowed: BTreeSet<Hash>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
) -> Policy {
    let (events, mut rx) = EventSender::channel(32, mask());
    let totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>> = Arc::default();
    let gate = Gate {
        allowed: Arc::new(allowed),
        expiry,
        done,
        store,
        totals: Arc::clone(&totals),
        touched: Arc::default(),
    };
    let task = AbortOnDropHandle::new(n0_future::task::spawn(async move {
        while let Some(message) = rx.recv().await {
            gate.answer(message).await;
        }
    }));
    Policy {
        events,
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
        throttle: Default::default(),
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
                let touched = Arc::clone(&self.touched);
                n0_future::task::spawn(async move {
                    while let Ok(Some(update)) = msg.rx.recv().await {
                        if let RequestUpdate::Completed(stats) = update {
                            let mut complete = whole;
                            if !complete {
                                complete = covered(
                                    &store,
                                    hash,
                                    stats.stats.payload_bytes_sent,
                                    &totals,
                                    connection,
                                )
                                .await;
                            }
                            lock(&touched).entry(connection).or_default().insert(hash);
                            if complete {
                                let _ = done.send(ServeEvent::Downloaded { hash, connection });
                            }
                            break;
                        }
                        if matches!(update, RequestUpdate::Aborted(_)) {
                            break;
                        }
                    }
                });
            }
            ProviderMessage::GetManyRequestReceived(msg) => {
                let ok = !expired(self.expiry)
                    && msg
                        .request
                        .hashes
                        .iter()
                        .all(|hash| self.allowed.contains(hash));
                msg.tx.send(answer(ok)).await.ok();
            }
            ProviderMessage::ObserveRequestReceived(msg) => {
                let ok = self.may(msg.request.hash);
                msg.tx.send(answer(ok)).await.ok();
            }
            ProviderMessage::ConnectionClosed(msg) => {
                let served = lock(&self.touched)
                    .remove(&msg.connection_id)
                    .unwrap_or_default();
                let _ = self.done.send(ServeEvent::Closed {
                    connection: msg.connection_id,
                    served,
                });
            }
            // Push is disabled in the mask, so a push is refused before this task sees it.
            _ => {}
        }
    }

    /// The one access rule: an allowed hash, before the ticket's deadline.
    fn may(&self, hash: Hash) -> bool {
        !expired(self.expiry) && self.allowed.contains(&hash)
    }
}
/// Whether the payload served for this connection and blob now adds up to the whole blob. A chunk
/// group request for a small file covers all of it; a request for the last block of a large one
/// does not. Asking twice for the same range could reach the size without the middle; the sender
/// only stops serving, and a client that wants to keep it alive asks for what it needs.
async fn covered(
    store: &FsStore,
    hash: Hash,
    payload: u64,
    totals: &Mutex<BTreeMap<(u64, Hash), u64>>,
    connection: u64,
) -> bool {
    let size = match store.blobs().status(hash).await {
        Ok(BlobStatus::Complete { size }) => size,
        _ => return false,
    };
    let mut totals = lock(totals);
    let total = totals.entry((connection, hash)).or_default();
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
