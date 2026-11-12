//! The delivery of a receipt, as `SPEC.md` section 10 states it: a receiver's request for the root
//! it received, and the sender's answer when its policy admitted that endpoint for that root.
//!
//! The request travels on a fresh `sluice/1` connection, after the two greetings: the content
//! connection is the blob protocol's and carries no stream of ours, and `Hello` is released, so it
//! cannot gain a field. A request is a record of its own, which leaves a 0.1.0 sender reading the
//! greeting and answering nothing, as section 10 asks.
//!
//! Nothing here can fail a transfer. The content was verified before a receipt is asked for, and
//! every refusal is an absence.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::Duration,
};

use ed25519_dalek::SigningKey;
use iroh_blobs::Hash;
use n0_future::time::timeout;
use tokio::sync::{Notify, mpsc::UnboundedSender};

use super::hello::{CAP_RECEIPTS, Hello, read_stream_varint, varint_bytes};
use super::policy::{Asked, Places};
use super::{Event, Receiver, now_ms};
use crate::receipt::Receipt;
use crate::ticket::Ticket;

mod serve;
mod write;

pub use serve::parse_request;
pub(in crate::engine) use serve::serve_request;
pub(super) use write::{ready, refuse_under_out, write_file};

/// How long a receiver waits for the answer, counted from the connection coming up.
const ANSWER_WAIT: Duration = Duration::from_secs(10);
/// The window between a receipt's time and the receiver's clock.
pub(super) const CLOCK_WINDOW_MS: u64 = 300_000;
/// How long a one-download sender keeps answering after the last byte of what it serves.
pub(crate) const HOLD: Duration = Duration::from_secs(10);

/// One open exchange of a sender, from the greeting that sets the bit to the answer being taken or
/// the request being refused. Dropping it ends the exchange and wakes the hold.
pub(crate) struct Pending<'a> {
    receipts: &'a Receipts,
    taken: bool,
}

impl Pending<'_> {
    /// Marks the answer as taken by the asker, so the drop counts it as delivered.
    pub(crate) fn taken(&mut self) {
        self.taken = true;
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if self.taken {
            self.receipts.answered.fetch_add(1, Ordering::SeqCst);
        }
        self.receipts.pending.fetch_sub(1, Ordering::SeqCst);
        self.receipts.changed.notify_one();
    }
}

/// What a sender signs for, once its ticket exists.
#[derive(Debug)]
struct Offer {
    root: [u8; 32],
    total_len: u64,
    key: SigningKey,
    expiry_ms: Option<u64>,
}

/// A sender's receipt state: what it serves, which endpoints its policy admitted, and how many
/// receipts it has written.
#[derive(Debug)]
pub(crate) struct Receipts {
    offer: Mutex<Option<Offer>>,
    /// Cleared by `--no-receipt` before the ticket is printed, and read again for every greeting.
    enabled: AtomicBool,
    asked: Arc<Mutex<Asked>>,
    places: Arc<Mutex<Places>>,
    answered: AtomicU32,
    /// Answers decided and not yet delivered or given up, which a sender keeps serving for.
    pending: AtomicU32,
    /// Wakes the sender's hold when an answer settles, or when `--no-receipt` clears the bit. One
    /// waiter reads it, and a stored permit means a wake between the hold's check and its wait is
    /// not lost.
    changed: Notify,
    /// How long a test makes the sender wait between deciding to answer and writing, so an answer
    /// that is late is reached through a real transfer.
    #[cfg(test)]
    delay: Mutex<Duration>,
    /// A record a test puts in place of the signed one, so each rule a receiver applies can be
    /// reached through a real transfer.
    #[cfg(test)]
    forced: Mutex<Option<Vec<u8>>>,
}

impl Receipts {
    /// A sender that offers nothing yet. The endpoint accepts connections before a ticket exists,
    /// so the state exists from the start and is filled in before the ticket is printed.
    pub(crate) fn new(asked: Arc<Mutex<Asked>>, places: Arc<Mutex<Places>>) -> Arc<Self> {
        Arc::new(Self {
            offer: Mutex::new(None),
            enabled: AtomicBool::new(false),
            asked,
            places,
            answered: AtomicU32::new(0),
            pending: AtomicU32::new(0),
            changed: Notify::new(),
            #[cfg(test)]
            delay: Mutex::new(Duration::ZERO),
            #[cfg(test)]
            forced: Mutex::new(None),
        })
    }

    /// Writes `record` in place of the signed one, for the next answers. An empty record writes
    /// nothing. A test only reaches this; a send has no flag for it.
    #[cfg(test)]
    pub(crate) fn force_record(&self, record: Vec<u8>) {
        *lock(&self.forced) = Some(record);
    }

    /// Makes the next answers wait `by` before they are written. A test only reaches this.
    #[cfg(test)]
    pub(crate) fn delay_answers(&self, by: Duration) {
        *lock(&self.delay) = by;
    }

    /// Offers a receipt for `root` and `total_len` bytes, signed with the key `seed` names. Called
    /// before the ticket that names them is printed.
    pub(crate) fn offer(
        &self,
        root: [u8; 32],
        total_len: u64,
        seed: [u8; 32],
        expiry_ms: Option<u64>,
    ) {
        *lock(&self.offer) = Some(Offer {
            root,
            total_len,
            key: SigningKey::from_bytes(&seed),
            expiry_ms,
        });
        self.enabled.store(true, Ordering::SeqCst);
    }

    /// Clears the bit and stops answering. A receipt already written cannot be recalled.
    pub(crate) fn disable(&self) {
        self.enabled.store(false, Ordering::SeqCst);
        self.changed.notify_one();
    }

    /// Whether this sender sets the bit in its greeting.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::SeqCst)
    }

    /// How many receipts this send has delivered: written, finished, and acknowledged by the asker,
    /// or left behind by an asker that closed the connection.
    pub(crate) fn answered(&self) -> u32 {
        self.answered.load(Ordering::SeqCst)
    }

    /// How many exchanges are open: a greeting that sets the bit was accepted and its request has
    /// not been refused, or its answer has not been taken.
    pub(crate) fn pending(&self) -> u32 {
        self.pending.load(Ordering::SeqCst)
    }

    /// Opens one exchange, which the sender keeps serving for until the guard is dropped. The guard
    /// is released on every path out of the exchange, an early return and a cancelled task included.
    pub(crate) fn begin(&self) -> Pending<'_> {
        self.pending.fetch_add(1, Ordering::SeqCst);
        Pending {
            receipts: self,
            taken: false,
        }
    }

    /// Waits until an answer settles or the bit is cleared. Whoever wakes re-reads the counts.
    pub(crate) async fn wait_changed(&self) {
        self.changed.notified().await;
    }

    /// The receipt for `requested_root`, when `peer` is an endpoint the policy admitted for that
    /// root: it asked for the root on a connection the policy let in, and on a limited send it
    /// holds a place. `now` is the signing time, which is also what the ticket's deadline is
    /// compared with. Every other request is answered with nothing. Nothing is counted here: the
    /// answer is counted when its [`Pending`] guard says the asker took it.
    pub(crate) fn answer(
        &self,
        peer: [u8; 32],
        requested_root: [u8; 32],
        now: u64,
    ) -> Option<Receipt> {
        if !self.enabled() {
            return None;
        }
        let offer = lock(&self.offer);
        let offer = offer.as_ref()?;
        if requested_root != offer.root {
            return None;
        }
        if offer.expiry_ms.is_some_and(|expiry| now >= expiry) {
            return None;
        }
        if !lock(&self.asked).peer_asked(&peer, Hash::from(offer.root)) {
            return None;
        }
        let places = lock(&self.places);
        if places.limit().is_some() && !places.holds(&peer) {
            return None;
        }
        drop(places);
        let receipt = Receipt::sign(
            &offer.key,
            crate::receipt::commitment(&offer.root),
            offer.total_len,
            now,
        );
        Some(receipt)
    }
}

/// Asks the sender of `ticket` for a receipt of the content that arrived, and returns it when one
/// passes every rule of section 10. `None` is the whole of a refusal, and a transfer never fails
/// over it.
pub(super) async fn ask(
    receiver: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    total_len: u64,
    events: &UnboundedSender<Event>,
) -> Option<Receipt> {
    ask_within(receiver, ticket, root, total_len, ANSWER_WAIT, events).await
}

/// [`ask`] with a bound on the whole exchange, so a test reaches the end of rule 6 quickly.
pub(crate) async fn ask_within(
    receiver: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    total_len: u64,
    wait: Duration,
    events: &UnboundedSender<Event>,
) -> Option<Receipt> {
    let conn = receiver
        .connect_alpn(ticket, super::ALPN, events.clone())
        .await
        .ok()?;
    let answer = timeout(
        wait,
        exchange(&conn, *ticket.endpoint_id(), root, total_len),
    )
    .await;
    conn.close(0u32.into(), b"");
    answer.ok().flatten()
}

/// The exchange on one connection: greet, ask, read one record, check it.
async fn exchange(
    conn: &iroh::endpoint::Connection,
    sender_key: [u8; 32],
    root: [u8; 32],
    total_len: u64,
) -> Option<Receipt> {
    let ours = Hello::ours().with(CAP_RECEIPTS);
    let (theirs, mut send, mut recv) = super::hello::greet_stream_with(conn, ours).await.ok()?;
    if !ours.both(&theirs, CAP_RECEIPTS) {
        return None;
    }
    let mut request = varint_bytes(serve::REQUEST_LEN);
    request.extend_from_slice(&root);
    send.write_all(&request).await.ok()?;
    send.finish().ok()?;
    let len = read_stream_varint(&mut recv).await.ok()?;
    // A record of another length is an absence, and nothing more is read from it: a peer that
    // writes a length it does not follow would otherwise make a receiver wait for its bytes.
    if len != crate::receipt::RECORD_LEN as u64 {
        return None;
    }
    let mut bytes = [0u8; crate::receipt::RECORD_LEN];
    recv.read_exact(&mut bytes).await.ok()?;
    accept(&bytes, &root, total_len, &sender_key, now_ms())
}

/// Every rule of section 10 that a received record must pass, as one decision. `None` is the
/// absence the receiver records.
pub(super) fn accept(
    bytes: &[u8],
    root: &[u8; 32],
    total_len: u64,
    sender_key: &[u8; 32],
    now: u64,
) -> Option<Receipt> {
    let receipt = Receipt::from_bytes(bytes).ok()?;
    if receipt.sender_key() != sender_key {
        return None;
    }
    if receipt.total_len() != total_len || !receipt.is_of(root) {
        return None;
    }
    if receipt.utc_ms().abs_diff(now) > CLOCK_WINDOW_MS {
        return None;
    }
    receipt.verify().ok()?;
    Some(receipt)
}

/// The lock cannot be held across an await and every holder only reads or replaces a value, so a
/// panic in one task poisoning it changes nothing here.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
