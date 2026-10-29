//! The listening provider: an endpoint, the one access policy for every request type, and the
//! stops a send uses.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{Endpoint, EndpointAddr, protocol::Router};
use iroh_blobs::{Hash, store::fs::FsStore};

use super::{endpoint, live::LiveLog, now_ms, policy, storage};
mod keep;
mod report;
mod start;
mod stop;
mod waiting;

use stop::{GRACE, wait_grace};
pub use stop::{ServeEnd, Stop};
use waiting::Waiting;

use crate::Error;

/// How long a send that has delivered every root waits for the last receiver's close report before
/// it stops. The report is made by another task, and without the wait the sender could miss its own
/// last line about a receiver.
const SETTLE: Duration = Duration::from_millis(250);

/// A listening provider. It holds the store open and answers only its allowed hashes.
pub struct Provider {
    router: Option<Router>,
    endpoint: Endpoint,
    store: FsStore,
    policy: policy::Policy,
    done: tokio::sync::mpsc::UnboundedReceiver<policy::ServeEvent>,
    /// Every content connection a receiver opened or closed, and every place taken or refused, in
    /// the order it happened. The sender is cloned into the endpoint's hook and into the gate, so
    /// the channel closes only when the endpoint's reporter and the policy are both gone.
    receivers: tokio::sync::mpsc::UnboundedReceiver<endpoint::Receiver>,
    /// The places a limited send offers. The policy decides them; this side reads them to know when
    /// the send has served everyone it promised to.
    places: Arc<Mutex<policy::Places>>,
    expiry: Option<u64>,
    /// The send's own folder, removed after this provider has closed its store. A provider that is
    /// dropped without being shut down hands the folder on with the store, so the two go in the
    /// order that works: the database is closed and the folder removed after it.
    _dir: Option<storage::Temp>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("endpoint", &self.endpoint.id().fmt_short().to_string())
            .finish_non_exhaustive()
    }
}

impl Drop for Provider {
    /// A provider that is dropped without being shut down has no moment to await the router's
    /// shutdown: a send that was cancelled, or one whose ticket could not be built. The store it
    /// holds is handed to the next command in this process, which closes it before it opens its
    /// own, so the database is free for the next send that imports into the same home.
    fn drop(&mut self) {
        if self.router.is_some() {
            storage::hand_on(Some(self.store.clone()), self._dir.take());
        }
    }
}

impl Provider {
    /// The address to put in a ticket.
    pub fn addr(&self) -> EndpointAddr {
        endpoint::ticket_addr(&self.endpoint)
    }

    /// Payload this provider sent of `hash`, for a test that checks what a request moved.
    #[cfg(test)]
    pub fn served_bytes(&self, hash: Hash) -> u64 {
        self.policy.served(hash)
    }

    /// The store, for an import before or a check after serving.
    pub fn store(&self) -> &FsStore {
        &self.store
    }

    /// Keeps the folder this send's store lives in, and removes it once the store is closed.
    pub(crate) fn hold(&mut self, dir: storage::Temp) {
        self._dir = Some(dir);
    }

    /// Sets how many places this send offers, and lets its requests in, before the ticket is
    /// printed.
    pub(super) fn set_limit(&mut self, limit: Option<u32>) {
        lock_places(&self.places).set_limit(limit);
    }

    /// Lets the requests of a send that offers no places in. A caller that never reserved has to
    /// be armed before it serves, or its first receiver would be refused.
    fn arm(&mut self) {
        lock_places(&self.places).arm();
    }

    /// The send's own folder, for a test that watches it while the send runs.
    #[cfg(test)]
    pub fn scratch(&self) -> Option<&std::path::Path> {
        self._dir.as_ref().map(storage::Temp::path)
    }

    /// Ends the receiver reports, as an endpoint that has gone would, so a test can prove that a
    /// wait ends with an error instead of polling a closed channel for ever.
    #[cfg(test)]
    pub fn stop_reports(&mut self) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        drop(sender);
        self.receivers = receiver;
    }

    /// Serves until the requested stop, reporting every receiver as a line of its own. The router is
    /// shut down before this returns, so a closed provider is not still accepting connections, and
    /// the store is closed after it, so the database is free for the next command that opens the
    /// same home.
    pub async fn serve(
        mut self,
        stop: Stop,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Result<ServeEnd, Error> {
        // The endpoint has been accepting since the send was prepared. The mode is set before the
        // first request can be answered: `reserve` does it earlier still, for a ticket that can be
        // dialled before this call, and a caller that skipped that is covered here.
        match &stop {
            Stop::Keep { limit, .. } => self.set_limit(*limit),
            _ => self.arm(),
        }
        let end = match stop {
            Stop::OneDownload(roots) => {
                self.one_download(roots.into_iter().map(Hash::from).collect(), events)
                    .await
            }
            Stop::Live(log) => self.live(log, events).await,
            Stop::Keep { limit, roots } => {
                self.keep(limit, roots.into_iter().map(Hash::from).collect(), events)
                    .await
            }
        };
        let closed = self.close().await;
        let end = end?;
        closed?;
        Ok(end)
    }

    /// Serves a live stream until its close is signed, then waits for the segments the way a
    /// static send waits for its roots. A reader can fetch a segment before the close, so the
    /// delivery events that arrived earlier are still in the channel when the wait starts.
    async fn live(
        &mut self,
        log: Arc<LiveLog>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Result<ServeEnd, Error> {
        loop {
            let left = self.expiry_ms_left();
            if left == 0 && self.expiry.is_some() {
                return Ok(ServeEnd::Expired);
            }
            let deadline = tokio::time::sleep(Duration::from_millis(left));
            tokio::pin!(deadline);
            tokio::select! {
                _ = log.wait_closed() => break,
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else {
                        return Err(reports_stopped());
                    };
                    self.report(reported, false, &BTreeSet::new(), events);
                }
                _ = &mut deadline => {
                    if self.expiry.is_none_or(|expiry| now_ms() >= expiry) {
                        return Ok(ServeEnd::Expired);
                    }
                }
            }
        }
        // A stream that ended with an error is not a finished send: the bytes a reader takes are
        // the ones sealed before it, so the sender reports the failure and stops.
        if let Some(message) = log.error() {
            return Err(Error::Storage(message));
        }
        let roots = log.roots();
        if roots.is_empty() {
            // An empty stream has no segment to fetch, so there is no delivery event to wait for.
            // The close is what a reader needed, and it has it.
            tokio::time::sleep(GRACE).await;
            return Ok(ServeEnd::Downloaded);
        }
        self.one_download(roots.into_iter().map(Hash::from).collect(), events)
            .await
    }

    /// Stops serving without waiting for a download.
    pub async fn shutdown(mut self) -> Result<(), Error> {
        self.close().await
    }

    /// Stops the router, and with it the blob protocol: shutting the router down is what closes
    /// the store, so the database is free for the next command that opens the same home.
    async fn close(&mut self) -> Result<(), Error> {
        let closed = self.shutdown_router().await;
        // The store is closed with the router, and the folder is removed after it, on a blocking
        // thread: a removal can wait for a file a scanner still holds, which is not work for an
        // async thread.
        if let Some(dir) = self._dir.take() {
            dir.remove_now().await;
        }
        closed
    }

    async fn shutdown_router(&mut self) -> Result<(), Error> {
        if let Some(router) = self.router.take() {
            router
                .shutdown()
                .await
                .map_err(|e| Error::Network(format!("cannot stop serving: {e}")))?;
        }
        Ok(())
    }

    async fn one_download(
        &mut self,
        roots: Vec<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) -> Result<ServeEnd, Error> {
        let mut wait = Waiting::new(roots)?;
        let left = self.expiry_ms_left();
        if left == 0 {
            return Ok(ServeEnd::Expired);
        }
        let mut deadline = Box::pin(tokio::time::sleep(Duration::from_millis(left)));
        let mut opened: BTreeSet<u64> = BTreeSet::new();
        loop {
            tokio::select! {
                // A receiver that verified and installed every file says so with the completion
                // code. It ends the wait even when a byte count would not: a resumed receive takes
                // only what the first one was missing, so this process never sees the whole blob.
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else {
                        return Err(reports_stopped());
                    };
                    Self::track(&reported, &mut opened);
                    let whole = self.completion(&reported, &wait.wanted);
                    self.report(reported, false, &wait.wanted, events);
                    if whole {
                        return Ok(ServeEnd::Downloaded);
                    }
                }
                event = self.done.recv() => match event {
                    Some(event) => wait.event(event),
                    None => return Err(Error::Bug("the provider event task stopped".into())),
                },
                // A receiver that keeps its connection open must not keep this sender alive: every
                // root was delivered, so stop after the grace period even without the close.
                _ = wait_grace(&mut wait.grace) => return Ok(ServeEnd::Downloaded),
                _ = deadline.as_mut() => {
                    if let Some(end) = elapsed(self.expiry, &mut deadline) {
                        return Ok(end);
                    }
                }
            }
            if wait.delivered() {
                // The bytes are out. A receiver's close report can still be on its way, and the
                // sender's last line about it is worth waiting a moment for, but not a second.
                self.settle(&mut opened, &wait.wanted, events).await;
                return Ok(ServeEnd::Downloaded);
            }
        }
    }

    /// Reports what is still on its way after the roots are delivered, for a moment at most.
    async fn settle(
        &mut self,
        opened: &mut BTreeSet<u64>,
        roots: &BTreeSet<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<crate::engine::Event>,
    ) {
        let deadline = tokio::time::sleep(SETTLE);
        tokio::pin!(deadline);
        while !opened.is_empty() {
            tokio::select! {
                _ = &mut deadline => break,
                reported = self.receivers.recv() => {
                    let Some(reported) = reported else { break };
                    Self::track(&reported, opened);
                    self.report(reported, false, roots, events);
                }
            }
        }
    }

    /// Whether a completion code from `connection` ends the wait. It is believed only for a
    /// connection the policy let a request in from that also asked for every root of the transfer:
    /// the code is unauthenticated, so anyone who can reach this sender could otherwise end a send
    /// without a ticket and make it print success.
    fn believed(&self, connection: u64, wanted: &BTreeSet<Hash>) -> bool {
        wanted.is_subset(&self.policy.asked_by(connection))
    }

    /// Whether a report is a completion code this provider believes: only a connection that asked
    /// for every root can end a wait with it, because the code itself is unauthenticated.
    fn completion(&self, reported: &endpoint::Receiver, roots: &BTreeSet<Hash>) -> bool {
        match reported {
            endpoint::Receiver::Closed {
                connection,
                claimed_whole: true,
                ..
            } => self.believed(*connection, roots),
            _ => false,
        }
    }
}

/// The endpoint's reports stopped without the endpoint being shut down. No path does that today:
/// the endpoint's hook and the request gate hold the senders, so their end means both are gone.
/// A wait that selected on a closed channel would poll it at once for ever, so it ends here.
fn reports_stopped() -> Error {
    Error::Bug("the endpoint stopped reporting its receivers".to_string())
}

/// The places lock, poisoned only by a panic that changed nothing it guards: a holder edits a set
/// and reads a clock.
fn lock_places(places: &Mutex<policy::Places>) -> std::sync::MutexGuard<'_, policy::Places> {
    places
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What a deadline that just fired means: another wait when the wall clock moved back, or the end
/// of the ticket. A sleep can wake early across a clock change, so the clock and not the timer
/// decides, and the remainder is waited for again.
fn elapsed(
    expiry: Option<u64>,
    deadline: &mut std::pin::Pin<Box<tokio::time::Sleep>>,
) -> Option<ServeEnd> {
    match expiry {
        Some(expiry) if now_ms() < expiry => {
            let left = Duration::from_millis(expiry - now_ms());
            deadline.as_mut().reset(tokio::time::Instant::now() + left);
            None
        }
        _ => Some(ServeEnd::Expired),
    }
}
