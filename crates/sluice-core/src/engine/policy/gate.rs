//! The gate: one request at a time, answered against the allowlist, the ticket deadline and the
//! places a limited send offers.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use iroh_blobs::{
    Hash,
    api::proto::BlobStatus,
    provider::events::{
        AbortReason, ConnectMode, EventMask, ObserveMode, ProviderMessage, RequestMode,
        RequestUpdate,
    },
    store::fs::FsStore,
};
use tokio::sync::mpsc::UnboundedSender;

use super::{
    ServeEvent,
    asked::Asked,
    delivery::{add, covers, credited, requested},
    lock,
    places::{Admission, Places},
};
use crate::engine::conn::{ConnIds, Peer};
use crate::engine::endpoint::Receiver;
use crate::engine::now_ms;

/// The byte ranges of one root this process has served, merged and in ascending order. A range
/// asked for twice counts once, so two requests for the first half are not the whole file.
pub(super) type Served = BTreeMap<Hash, Vec<(u64, u64)>>;

/// What every request is answered against: the allowlist, the ticket deadline, the reporter, and
/// the maps that say how much of each blob this process has served.
pub(super) struct Gate {
    /// Shared with a live send, which adds each segment as it seals it.
    allowed: Arc<Mutex<BTreeSet<Hash>>>,
    expiry: Option<u64>,
    done: UnboundedSender<ServeEvent>,
    store: FsStore,
    totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
    served: Arc<Mutex<Served>>,
    asked: Arc<Mutex<Asked>>,
    ids: Arc<ConnIds>,
    rate: Option<Arc<crate::engine::Rate>>,
    places: Arc<Mutex<Places>>,
    /// Where a place or a refusal is reported, on the same channel as a connection's open and close
    /// so the three keep their order.
    receivers: UnboundedSender<Receiver>,
}

impl Gate {
    /// The gate one provider answers from, with the maps it shares with the policy's owner.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        allowed: Arc<Mutex<BTreeSet<Hash>>>,
        expiry: Option<u64>,
        done: UnboundedSender<ServeEvent>,
        store: FsStore,
        totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>>,
        served: Arc<Mutex<Served>>,
        asked: Arc<Mutex<Asked>>,
        ids: Arc<ConnIds>,
        rate: Option<Arc<crate::engine::Rate>>,
        places: Arc<Mutex<Places>>,
        receivers: UnboundedSender<Receiver>,
    ) -> Self {
        Self {
            allowed,
            expiry,
            done,
            store,
            totals,
            served,
            asked,
            ids,
            rate,
            places,
            receivers,
        }
    }

    /// Answers one request. A get is checked against the allowlist, then its events decide when
    /// every byte of the blob went out.
    pub(super) async fn answer(&self, message: ProviderMessage) {
        if matches!(&message, ProviderMessage::GetRequestReceived(_)) {
            self.get_request(message).await;
        } else if matches!(&message, ProviderMessage::GetManyRequestReceived(_)) {
            self.get_many(message).await;
        } else if matches!(&message, ProviderMessage::ObserveRequestReceived(_)) {
            self.observe(message).await;
        } else if matches!(&message, ProviderMessage::ConnectionClosed(_)) {
            self.connection_closed(message);
        } else if matches!(&message, ProviderMessage::Throttle(_)) {
            self.throttle(message);
        }
    }

    /// One get: its permission, and the task that watches what it sends.
    async fn get_request(&self, message: ProviderMessage) {
        let ProviderMessage::GetRequestReceived(mut msg) = message else {
            unreachable!("the caller passes a get request")
        };
        let hash = msg.request.hash;
        // The id this process gave the connection. A connection the handshake never saw has none,
        // so its request is still checked against the allowlist but counted for no completion: an
        // id inherited from a closed connection would let a peer end a send it never received.
        let connection = self.ids.own(msg.connection_id).unwrap_or_default();
        let peer = self.ids.peer(connection);
        let ok = self.may(peer, hash);
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
        let places = Arc::clone(&self.places);
        let endpoint = peer.map(|peer| peer.endpoint);
        n0_future::task::spawn(async move {
            let sent = Sent {
                store: &store,
                totals: &totals,
                served: &served,
                connection,
            };
            while let Ok(Some(update)) = msg.rx.recv().await {
                // A holder that is moving bytes has not gone quiet, so it is not a connection that
                // does nothing.
                if let Some(endpoint) = endpoint {
                    lock(&places).touch(&endpoint, now_ms());
                }
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

    /// One request for several blobs at once, which is refused as a whole when one is not allowed.
    async fn get_many(&self, message: ProviderMessage) {
        let ProviderMessage::GetManyRequestReceived(msg) = message else {
            unreachable!("the caller passes a get-many request")
        };
        let peer = self
            .ids
            .peer(self.ids.own(msg.connection_id).unwrap_or_default());
        let ok = self.may_all(peer, &msg.request.hashes);
        msg.tx.send(answer(ok)).await.ok();
    }

    /// One observe request, which is the same rule as a get.
    async fn observe(&self, message: ProviderMessage) {
        let ProviderMessage::ObserveRequestReceived(msg) = message else {
            unreachable!("the caller passes an observe request")
        };
        let peer = self
            .ids
            .peer(self.ids.own(msg.connection_id).unwrap_or_default());
        let ok = self.may(peer, msg.request.hash);
        msg.tx.send(answer(ok)).await.ok();
    }

    /// A connection that closed, told to the send so a wait can end on it.
    fn connection_closed(&self, message: ProviderMessage) {
        let ProviderMessage::ConnectionClosed(msg) = message else {
            unreachable!("the caller passes a close")
        };
        if let Some(connection) = self.ids.own(msg.connection_id) {
            let _ = self.done.send(ServeEvent::Closed { connection });
        }
    }

    /// One paced chunk. The wait is a task of its own, not this gate's: one paced chunk would
    /// otherwise hold up the permission checks and close events of every other connection. The
    /// provider waits for the answer before it writes, so the pacing still applies.
    fn throttle(&self, message: ProviderMessage) {
        let ProviderMessage::Throttle(msg) = message else {
            unreachable!("the caller passes a throttle request")
        };
        let rate = self.rate.clone();
        n0_future::task::spawn(async move {
            if let Some(rate) = rate {
                rate.acquire(msg.size).await;
            }
            let _ = msg.tx.send(Ok(())).await;
        });
    }

    /// The one access rule: an allowed hash, before the ticket's deadline, and a place when the send
    /// offers them. The place is decided here, where the answer is, so a receiver the limit cannot
    /// take never reaches the protocol at all.
    fn may(&self, peer: Option<Peer>, hash: Hash) -> bool {
        if expired(self.expiry) || !lock(&self.allowed).contains(&hash) {
            return false;
        }
        self.admit(peer)
    }

    /// The same rule for a request that names several hashes at once: every one has to be allowed,
    /// so one refused hash refuses the whole request.
    fn may_all(&self, peer: Option<Peer>, hashes: &[Hash]) -> bool {
        if expired(self.expiry) {
            return false;
        }
        let allowed = lock(&self.allowed);
        if !hashes.iter().all(|hash| allowed.contains(hash)) {
            return false;
        }
        drop(allowed);
        self.admit(peer)
    }

    /// Admits or refuses one request by the places the send offers. A connection without a handshake
    /// record holds no place and can take none, so a limited send refuses it: an id this process
    /// never gave cannot be the same receiver twice. Until the send's mode is set, every request is
    /// refused and none takes a place.
    fn admit(&self, peer: Option<Peer>) -> bool {
        let now = now_ms();
        let mut places = lock(&self.places);
        let Some(peer) = peer else {
            return places.admits_unknown();
        };
        let admission = places.admit(peer.endpoint, now);
        let places_count = places.limit().unwrap_or_default() as u32;
        drop(places);
        match admission {
            Admission::Served => true,
            Admission::Took => {
                let _ = self.receivers.send(Receiver::Placed {
                    endpoint: peer.endpoint,
                    relayed: peer.relayed,
                });
                true
            }
            Admission::Refused => {
                let _ = self.receivers.send(Receiver::Refused {
                    endpoint: peer.endpoint,
                    relayed: peer.relayed,
                    places: places_count,
                });
                false
            }
            Admission::Unreported => false,
        }
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

/// Whether the send may answer at all, and how the event task watches it: with a cap, every chunk
/// is paced here; without one, paying a round trip per 16 KiB chunk would be for nothing.
pub(super) fn mask(limited: bool) -> EventMask {
    EventMask {
        connected: ConnectMode::Notify,
        get: RequestMode::InterceptLog,
        get_many: RequestMode::Intercept,
        push: RequestMode::Disabled,
        observe: ObserveMode::Intercept,
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

fn expired(expiry: Option<u64>) -> bool {
    expired_at(expiry, now_ms())
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
