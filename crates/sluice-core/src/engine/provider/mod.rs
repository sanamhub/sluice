//! The listening provider: an endpoint, the one access policy for every request type, and the
//! stops a send uses.

use std::{collections::BTreeSet, sync::Arc, time::Duration};

use iroh::{Endpoint, EndpointAddr, protocol::Router};
use iroh_blobs::{Hash, store::fs::FsStore};

use super::{endpoint, live::LiveLog, now_ms, policy};
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
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("endpoint", &self.endpoint.id().fmt_short().to_string())
            .finish_non_exhaustive()
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

    /// Serves until the requested stop. The router is shut down before this returns, so a closed
    /// provider is not still accepting connections.
    pub async fn serve(mut self, stop: Stop) -> Result<ServeEnd, Error> {
        let end = match stop {
            Stop::OneDownload(roots) => {
                self.one_download(roots.into_iter().map(Hash::from).collect())
                    .await?
            }
            Stop::Live(log) => self.live(log).await?,
            Stop::Keep => self.keep().await?,
        };
        self.shutdown_router().await?;
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
        self.shutdown_router().await
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
        let mut remaining: BTreeSet<Hash> = roots.into_iter().collect();
        if remaining.is_empty() {
            return Err(Error::Bug(
                "a one-download send has no content to serve".to_string(),
            ));
        }
        let left = self.expiry_ms_left();
        if left == 0 {
            return Ok(ServeEnd::Expired);
        }
        // Kept whole because `remaining` shrinks: a completion code is checked against every root
        // of the transfer, not only the ones still outstanding.
        let wanted = remaining.clone();
        let mut deadline = Box::pin(tokio::time::sleep(Duration::from_millis(left)));
        // Started when the last root is delivered and never restarted, so a client that keeps
        // asking for something else cannot hold the sender open for ever.
        let mut grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = None;
        let mut closed = BTreeSet::new();
        let mut connections = BTreeSet::new();
        loop {
            tokio::select! {
                // The receiver verified and installed every file, which is the signal that says so.
                // It ends the wait even when a byte count would not: a resumed receive takes only
                // what the first one was missing, so this process never sees the whole blob. It is
                // believed only for a connection that asked for every root: the close code is
                // unauthenticated, so anyone who can reach this sender could otherwise end a send
                // without a ticket and make it print success.
                Some(connection) = self.complete.recv() => {
                    if wanted.is_subset(&self.policy.asked_by(connection)) {
                        return Ok(ServeEnd::Downloaded);
                    }
                }
                event = self.done.recv() => match event {
                    Some(policy::ServeEvent::Downloaded { hash, connection }) => {
                        if remaining.remove(&hash) {
                            connections.insert(connection);
                        }
                        if remaining.is_empty() && grace.is_none() {
                            grace = Some(Box::pin(tokio::time::sleep(GRACE)));
                        }
                    }
                    Some(policy::ServeEvent::Closed { connection }) => {
                        closed.insert(connection);
                    }
                    None => return Err(Error::Bug("the provider event task stopped".into())),
                },
                // A receiver that keeps its connection open must not keep this sender alive: every
                // root was delivered, so stop after the grace period even without the close.
                _ = wait_grace(&mut grace) => return Ok(ServeEnd::Downloaded),
                _ = deadline.as_mut() => {
                    match self.expiry {
                        // The sleep can wake early when the wall clock moved back, so the clock
                        // and not the timer decides. The remainder is waited for again.
                        Some(expiry) if now_ms() < expiry => {
                            let left = Duration::from_millis(expiry - now_ms());
                            deadline.as_mut().reset(tokio::time::Instant::now() + left);
                        }
                        _ => return Ok(ServeEnd::Expired),
                    }
                }
            }
            if remaining.is_empty() && connections.is_subset(&closed) {
                return Ok(ServeEnd::Downloaded);
            }
        }
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
