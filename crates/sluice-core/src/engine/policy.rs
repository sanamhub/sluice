//! The one access policy of a provider. It answers `get`, `get_many` and `observe` with the same
//! set of allowed hashes, refuses every push, and refuses everything after the ticket expiry.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use iroh_blobs::{
    Hash,
    api::proto::BlobStatus,
    protocol::ChunkRangesSeq,
    provider::events::{
        AbortReason, ConnectMode, EventMask, EventSender, ObserveMode, ProviderMessage,
        RequestMode, RequestUpdate,
    },
    store::fs::FsStore,
};
use n0_future::task::AbortOnDropHandle;
use tokio::sync::mpsc::UnboundedSender;

/// The byte ranges of one root this process has served, merged and in ascending order. A range
/// asked for twice counts once, so two requests for the first half are not the whole file.
type Served = BTreeMap<Hash, Vec<(u64, u64)>>;

/// How many separate ranges one root may be accounted in. A client that asks for one range at a
/// time could otherwise grow this map without bound; past the cap the root is simply never
/// counted as delivered, which leaves the sender serving rather than stopping early.
const MAX_SPANS: usize = 4096;

/// The provider's policy and the task that answers its request events.
pub struct Policy {
    /// Handed to `BlobsProtocol`, which uses it for every request.
    pub events: EventSender,
    /// Payload served per connection and blob, so a test can see what a request moved.
    #[cfg(test)]
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
) -> Policy {
    let (events, mut rx) = EventSender::channel(32, mask(rate.is_some()));
    let totals: Arc<Mutex<BTreeMap<(u64, Hash), u64>>> = Arc::default();
    let gate = Gate {
        allowed,
        expiry,
        done,
        store,
        totals: Arc::clone(&totals),
        served: Arc::default(),
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
                let connection = msg.connection_id;
                let whole = msg.request.ranges.is_all();
                let asked = requested(&msg.request.ranges);
                if msg.tx.send(answer(ok)).await.is_err() || !ok {
                    return;
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
                        let covered = match update {
                            // Everything the request asked for went out.
                            RequestUpdate::Completed(stats) => {
                                whole
                                    || sent
                                        .cover(hash, &asked, stats.stats.payload_bytes_sent, true)
                                        .await
                            }
                            // A request that stopped part way still moved the bytes it sent, and a
                            // resume asks only for the rest, so they count towards the whole.
                            RequestUpdate::Aborted(stats) => {
                                !whole
                                    && sent
                                        .cover(hash, &asked, stats.stats.payload_bytes_sent, false)
                                        .await
                            }
                            _ => continue,
                        };
                        if covered {
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
                let _ = self.done.send(ServeEvent::Closed {
                    connection: msg.connection_id,
                });
            }
            ProviderMessage::Throttle(msg) => {
                if let Some(rate) = &self.rate {
                    rate.acquire(msg.size).await;
                }
                msg.tx.send(Ok(())).await.ok();
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
    /// Whether the ranges this process served for `hash` now reach the whole blob. `sent_bytes` is
    /// what the request moved before it ended: a completed request sent everything it asked for,
    /// and an aborted one sent that many bytes from the front of what it asked for, which is the
    /// order a provider writes in. The count is per root and not per connection, so a receive that
    /// fetches half a file, is killed and fetches the other half still finishes the root. It is a
    /// fallback for a receiver that never says it is done, and it can only be wrong in the
    /// direction of serving longer.
    async fn cover(
        &self,
        hash: Hash,
        asked: &[(u64, u64)],
        sent_bytes: u64,
        complete: bool,
    ) -> bool {
        let size = match self.store.blobs().status(hash).await {
            Ok(BlobStatus::Complete { size }) => size,
            _ => return false,
        };
        {
            let mut totals = lock(self.totals);
            let sent = totals.entry((self.connection, hash)).or_default();
            *sent = sent.saturating_add(sent_bytes);
        }
        if size == 0 {
            return complete;
        }
        let asked = clamp(asked, size);
        let asked = if complete {
            asked
        } else {
            front(asked, sent_bytes)
        };
        let mut served = lock(self.served);
        let spans = served.entry(hash).or_default();
        for (start, end) in asked {
            add(spans, start, end);
        }
        covers(spans, size)
    }
}

/// The chunk number a request uses for "the last chunk of this blob", before it knows how large the
/// blob is. It means the blob's final chunk, which is the only part of it such a request can name.
const LAST_CHUNK: u64 = u64::MAX;

/// The size of one `iroh-blobs` chunk, which is what a chunk range counts in.
const CHUNK_BYTES: u64 = 1024;

/// The byte ranges a request asked for, as it asked them. A get names one blob, so the ranges of
/// blob 0 are the ones it serves; a request that names another blob as well is not one a get makes
/// and it is not counted.
fn requested(ranges: &ChunkRangesSeq) -> Vec<(u64, u64)> {
    use std::ops::{Bound, RangeBounds};

    let mut out = Vec::new();
    for range in ranges[0].iter() {
        let (start, end) = match (range.start_bound(), range.end_bound()) {
            (Bound::Included(chunk), _) if chunk.0 == LAST_CHUNK => {
                // The last-chunk marker is the whole range, so it has no start below its end.
                out.push((LAST_CHUNK, LAST_CHUNK));
                continue;
            }
            (Bound::Included(chunk), Bound::Excluded(end)) => (chunk.to_bytes(), end.to_bytes()),
            (Bound::Included(chunk), Bound::Unbounded) => (chunk.to_bytes(), u64::MAX),
            (Bound::Included(chunk), Bound::Included(_)) => (
                chunk.to_bytes(),
                chunk.to_bytes().saturating_add(CHUNK_BYTES),
            ),
            _ => continue,
        };
        if start < end {
            out.push((start, end));
        }
    }
    out
}

/// The same ranges clamped to a blob of `size` bytes. The last-chunk marker becomes the blob's
/// final chunk.
fn clamp(ranges: &[(u64, u64)], size: u64) -> Vec<(u64, u64)> {
    ranges
        .iter()
        .map(|(start, end)| {
            if *start == LAST_CHUNK {
                (size.saturating_sub(CHUNK_BYTES), size)
            } else {
                ((*start).min(size), (*end).min(size))
            }
        })
        .filter(|(start, end)| start < end)
        .collect()
}

/// The first `sent` bytes of `ranges`, which is what an aborted request got through.
fn front(ranges: Vec<(u64, u64)>, sent: u64) -> Vec<(u64, u64)> {
    let mut left = sent;
    let mut out = Vec::new();
    for (start, end) in ranges {
        if left == 0 {
            break;
        }
        let take = (end - start).min(left);
        out.push((start, start + take));
        left -= take;
    }
    out
}

/// Adds one range to a merged, ascending list of them.
fn add(spans: &mut Vec<(u64, u64)>, start: u64, end: u64) {
    if start >= end || spans.len() >= MAX_SPANS {
        return;
    }
    spans.push((start, end));
    spans.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(spans.len());
    for (start, end) in spans.drain(..) {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    *spans = merged;
}

/// Whether the served ranges are exactly the whole blob, in one span.
fn covers(spans: &[(u64, u64)], size: u64) -> bool {
    spans == [(0, size)]
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
    use super::{add, clamp, covers, expired_at, front, requested};
    use iroh_blobs::protocol::{ChunkRanges, ChunkRangesExt, ChunkRangesSeq};

    fn ask(ranges: ChunkRanges) -> Vec<(u64, u64)> {
        requested(&ChunkRangesSeq::from_ranges([ranges]))
    }

    fn whole(size: u64) -> Vec<(u64, u64)> {
        clamp(&ask(ChunkRanges::all()), size)
    }

    #[test]
    fn the_boundary_is_the_ticket_boundary() {
        assert!(!expired_at(None, u64::MAX));
        assert!(expired_at(Some(0), 0));
        assert!(!expired_at(Some(10), 9));
        assert!(expired_at(Some(10), 10));
        assert!(expired_at(Some(10), 11));
    }

    #[test]
    fn asking_twice_for_the_first_half_is_not_the_whole_file() {
        let half = clamp(&ask(ChunkRanges::bytes(0..65_536)), 131_072);
        let mut spans = Vec::new();
        for _ in 0..2 {
            for (start, end) in half.clone() {
                add(&mut spans, start, end);
            }
        }
        assert!(!covers(&spans, 131_072), "{spans:?}");
    }

    #[test]
    fn the_two_halves_add_up_to_the_whole_file() {
        let mut spans = Vec::new();
        for (start, end) in clamp(&ask(ChunkRanges::bytes(0..65_536)), 131_072) {
            add(&mut spans, start, end);
        }
        for (start, end) in clamp(&ask(ChunkRanges::bytes(65_536..131_072)), 131_072) {
            add(&mut spans, start, end);
        }
        assert!(covers(&spans, 131_072), "{spans:?}");
    }

    #[test]
    fn an_open_range_reaches_the_end_of_the_blob() {
        assert_eq!(whole(4096), vec![(0, 4096)]);
    }

    #[test]
    fn an_aborted_request_counts_what_it_sent() {
        assert_eq!(
            front(vec![(0, 1000), (2000, 3000)], 1500),
            vec![(0, 1000), (2000, 2500)]
        );
        assert_eq!(front(vec![(0, 1000)], 0), Vec::new());
    }
}
