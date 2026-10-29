//! The listening provider: an endpoint, the one access policy for every request type, and the
//! stops a send uses.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use iroh::{Endpoint, EndpointAddr, protocol::Router};
use iroh_blobs::{Hash, store::fs::FsStore};

use super::{endpoint, live::LiveLog, now_ms, policy, storage};
mod start;
mod stop;

use stop::{GRACE, wait_grace};
pub use stop::{ServeEnd, Stop};

use crate::Error;

/// A listening provider. It holds the store open and answers only its allowed hashes.
pub struct Provider {
    router: Option<Router>,
    endpoint: Endpoint,
    store: FsStore,
    policy: policy::Policy,
    done: tokio::sync::mpsc::UnboundedReceiver<policy::ServeEvent>,
    /// One message per connection that a receiver closed with the completion code, holding that
    /// connection. The sender end is kept so the channel never closes while this provider is alive.
    complete: tokio::sync::mpsc::UnboundedReceiver<u64>,
    _complete: tokio::sync::mpsc::UnboundedSender<u64>,
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

    /// The send's own folder, for a test that watches it while the send runs.
    #[cfg(test)]
    pub fn scratch(&self) -> Option<&std::path::Path> {
        self._dir.as_ref().map(storage::Temp::path)
    }

    /// Serves until the requested stop. The router is shut down before this returns, so a closed
    /// provider is not still accepting connections, and the store is closed after it, so the
    /// database is free for the next command that opens the same home.
    pub async fn serve(mut self, stop: Stop) -> Result<ServeEnd, Error> {
        let end = match stop {
            Stop::OneDownload(roots) => {
                self.one_download(roots.into_iter().map(Hash::from).collect())
                    .await
            }
            Stop::Live(log) => self.live(log).await,
            Stop::Keep => self.keep().await,
        };
        let closed = self.close().await;
        let end = end?;
        closed?;
        Ok(end)
    }

    /// Serves a live stream until its close is signed, then waits for the segments the way a
    /// static send waits for its roots. A reader can fetch a segment before the close, so the
    /// delivery events that arrived earlier are still in the channel when the wait starts.
    async fn live(&mut self, log: Arc<LiveLog>) -> Result<ServeEnd, Error> {
        loop {
            let left = self.expiry_ms_left();
            if left == 0 && self.expiry.is_some() {
                return Ok(ServeEnd::Expired);
            }
            let deadline = tokio::time::sleep(Duration::from_millis(left));
            tokio::pin!(deadline);
            tokio::select! {
                _ = log.wait_closed() => break,
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
        self.one_download(roots.into_iter().map(Hash::from).collect())
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

    async fn one_download(&mut self, roots: Vec<Hash>) -> Result<ServeEnd, Error> {
        let mut wait = Waiting::new(roots)?;
        let left = self.expiry_ms_left();
        if left == 0 {
            return Ok(ServeEnd::Expired);
        }
        let mut deadline = Box::pin(tokio::time::sleep(Duration::from_millis(left)));
        loop {
            tokio::select! {
                // The receiver verified and installed every file, which is the signal that says so.
                // It ends the wait even when a byte count would not: a resumed receive takes only
                // what the first one was missing, so this process never sees the whole blob.
                Some(connection) = self.complete.recv() => {
                    if self.believed(connection, &wait.wanted) {
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
                return Ok(ServeEnd::Downloaded);
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

    async fn keep(&mut self) -> Result<ServeEnd, Error> {
        loop {
            let left = self.expiry_ms_left();
            if left == 0 && self.expiry.is_some() {
                return Ok(ServeEnd::Expired);
            }
            let deadline = tokio::time::sleep(Duration::from_millis(left));
            tokio::pin!(deadline);
            tokio::select! {
                _ = tokio::signal::ctrl_c() => return Ok(ServeEnd::Interrupted),
                _ = &mut deadline => {
                    // The timer is not the clock: a sleep can end early across a clock change.
                    if self.expiry.is_none_or(|expiry| now_ms() >= expiry) {
                        return Ok(ServeEnd::Expired);
                    }
                }
            }
        }
    }

    /// Milliseconds until the ticket deadline, or the far future when the ticket has none.
    fn expiry_ms_left(&self) -> u64 {
        match self.expiry {
            Some(expiry) => expiry.saturating_sub(now_ms()),
            None => 100 * 365 * 24 * 60 * 60 * 1000,
        }
    }
}

/// What a one-download wait knows: the roots still owed, every root of the transfer (a completion
/// code is checked against all of them, not only the outstanding ones), the grace that starts when
/// the last root goes out, and which connections delivered or closed.
struct Waiting {
    remaining: BTreeSet<Hash>,
    wanted: BTreeSet<Hash>,
    grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    closed: BTreeSet<u64>,
    connections: BTreeSet<u64>,
}

impl Waiting {
    fn new(roots: Vec<Hash>) -> Result<Self, Error> {
        let remaining: BTreeSet<Hash> = roots.into_iter().collect();
        if remaining.is_empty() {
            return Err(Error::Bug(
                "a one-download send has no content to serve".to_string(),
            ));
        }
        Ok(Self {
            wanted: remaining.clone(),
            remaining,
            grace: None,
            closed: BTreeSet::new(),
            connections: BTreeSet::new(),
        })
    }

    /// One policy event: a delivered root is no longer owed, and a closed connection is recorded so
    /// the wait can end once every connection that delivered a root has gone. The grace is started
    /// when the last root goes out and never restarted, so a client that keeps asking for something
    /// else cannot hold the sender open for ever.
    fn event(&mut self, event: policy::ServeEvent) {
        match event {
            policy::ServeEvent::Downloaded { hash, connection } => {
                if self.remaining.remove(&hash) {
                    self.connections.insert(connection);
                }
                if self.remaining.is_empty() && self.grace.is_none() {
                    self.grace = Some(Box::pin(tokio::time::sleep(GRACE)));
                }
            }
            policy::ServeEvent::Closed { connection } => {
                self.closed.insert(connection);
            }
        }
    }

    /// Whether every root is delivered and every connection that took one has closed: nothing is
    /// left to ask for, and nobody is left who could.
    fn delivered(&self) -> bool {
        self.remaining.is_empty() && self.connections.is_subset(&self.closed)
    }
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
