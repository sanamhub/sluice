//! The listening provider: an endpoint, the one access policy for every request type, and the
//! stops a send uses.

use std::{
    collections::BTreeSet,
    ops::Deref,
    sync::{Arc, Mutex},
    time::Duration,
};

use iroh::{Endpoint, EndpointAddr, protocol::Router};
use iroh_blobs::{BlobsProtocol, Hash, store::fs::FsStore};
use tokio::sync::mpsc::unbounded_channel;

use super::{
    ALPN, Congestion, Rate, RelayModeChoice, SluiceProtocol, endpoint, live::LiveLog, now_ms,
    policy,
};
mod stop;

use stop::{GRACE, wait_grace};
pub use stop::{ServeEnd, Stop};

use crate::{Error, Home};

/// Binds the provider's endpoint, with the identity key of `home`, and the completion signal a
/// one-download stop watches. A named port that cannot be bound is an error the user can act on; a
/// saved port that is taken falls back to any port, because a ticket is better than a refusal.
async fn open_endpoint(
    home: &Home,
    relay: &RelayModeChoice,
    congestion: Congestion,
    port: Option<u16>,
    complete: tokio::sync::mpsc::UnboundedSender<()>,
) -> Result<Endpoint, Error> {
    let seed = home.identity()?.secret_seed();
    let saved = home.saved_port().filter(|_| port.is_none());
    let chosen = port.or(saved);
    let first =
        endpoint::bind_with_completion(Some(seed), relay, congestion, chosen, complete.clone());
    match first.await {
        Ok(endpoint) => Ok(endpoint),
        Err(e) if port.is_some() => {
            let port = port.unwrap_or_default();
            Err(Error::Storage(format!("cannot bind UDP port {port}: {e}")))
        }
        Err(_) if saved.is_some() => {
            endpoint::bind_with_completion(Some(seed), relay, congestion, None, complete).await
        }
        Err(e) => Err(e),
    }
}
/// A listening provider. It holds the store open and answers only its allowed hashes.
pub struct Provider {
    router: Option<Router>,
    endpoint: Endpoint,
    store: FsStore,
    _policy: policy::Policy,
    done: tokio::sync::mpsc::UnboundedReceiver<policy::ServeEvent>,
    /// One message per connection that a receiver closed with the completion code. The sender end
    /// is kept so the channel never closes while this provider is alive.
    complete: tokio::sync::mpsc::UnboundedReceiver<()>,
    _complete: tokio::sync::mpsc::UnboundedSender<()>,
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
    /// Binds the sender's endpoint from the home's identity and starts the blob protocol with one
    /// access policy for every request type. `port` names the UDP port to bind, so a firewall rule
    /// can name it; without it the port this home used last is tried first.
    pub async fn start(
        home: &Home,
        store: FsStore,
        allowed: BTreeSet<[u8; 32]>,
        relay: RelayModeChoice,
        congestion: Congestion,
        expiry: Option<u64>,
        port: Option<u16>,
    ) -> Result<Self, Error> {
        let allowed: BTreeSet<Hash> = allowed.into_iter().map(Hash::from).collect();
        let allowed = Arc::new(Mutex::new(allowed));
        Self::start_inner(
            home, store, allowed, None, relay, congestion, expiry, port, None,
        )
        .await
    }

    /// [`start`] with a byte rate for serving, which enables the provider throttle.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_with_rate(
        home: &Home,
        store: FsStore,
        allowed: BTreeSet<[u8; 32]>,
        relay: RelayModeChoice,
        congestion: Congestion,
        expiry: Option<u64>,
        port: Option<u16>,
        rate: Option<Arc<Rate>>,
    ) -> Result<Self, Error> {
        let allowed: BTreeSet<Hash> = allowed.into_iter().map(Hash::from).collect();
        let allowed = Arc::new(Mutex::new(allowed));
        Self::start_inner(
            home, store, allowed, None, relay, congestion, expiry, port, rate,
        )
        .await
    }

    /// Starts a provider whose `sluice/1` connection serves `log`, and whose static policy answers
    /// for each segment the log seals. The allowlist is the log's, so a segment becomes readable
    /// at the moment the frame that names it is pushed.
    pub async fn start_live(
        home: &Home,
        store: FsStore,
        log: Arc<LiveLog>,
        relay: RelayModeChoice,
        congestion: Congestion,
        expiry: Option<u64>,
        port: Option<u16>,
    ) -> Result<Self, Error> {
        let allowed = log.allowed();
        Self::start_inner(
            home,
            store,
            allowed,
            Some(log),
            relay,
            congestion,
            expiry,
            port,
            None,
        )
        .await
    }

    /// [`start_live`] with a byte rate, which enables the provider throttle.
    #[allow(clippy::too_many_arguments)]
    pub async fn start_live_with_rate(
        home: &Home,
        store: FsStore,
        log: Arc<LiveLog>,
        relay: RelayModeChoice,
        congestion: Congestion,
        expiry: Option<u64>,
        port: Option<u16>,
        rate: Option<Arc<Rate>>,
    ) -> Result<Self, Error> {
        let allowed = log.allowed();
        Self::start_inner(
            home,
            store,
            allowed,
            Some(log),
            relay,
            congestion,
            expiry,
            port,
            rate,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_inner(
        home: &Home,
        store: FsStore,
        allowed: Arc<Mutex<BTreeSet<Hash>>>,
        live: Option<Arc<LiveLog>>,
        relay: RelayModeChoice,
        congestion: Congestion,
        expiry: Option<u64>,
        port: Option<u16>,
        rate: Option<Arc<Rate>>,
    ) -> Result<Self, Error> {
        let (complete, complete_rx) = unbounded_channel();
        let endpoint = open_endpoint(home, &relay, congestion, port, complete.clone()).await?;
        if let Some(port) = endpoint
            .bound_sockets()
            .first()
            .map(std::net::SocketAddr::port)
        {
            home.save_port(port);
        }
        if !matches!(relay, RelayModeChoice::Disabled) {
            endpoint::wait_for_online(&endpoint).await?;
        }
        endpoint::wait_for_direct_addr(&endpoint).await?;
        let (done, done_rx) = unbounded_channel();
        let policy = policy::start_shared(allowed, expiry, done, store.clone(), rate);
        let blobs = BlobsProtocol::new(store.deref(), Some(policy.events.clone()));
        let sluice = match live {
            Some(log) => SluiceProtocol::live(log),
            None => SluiceProtocol::default(),
        };
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(ALPN, sluice)
            .spawn();
        Ok(Self {
            router: Some(router),
            endpoint,
            store,
            _policy: policy,
            done: done_rx,
            complete: complete_rx,
            _complete: complete,
            expiry,
        })
    }

    /// The address to put in a ticket.
    pub fn addr(&self) -> EndpointAddr {
        endpoint::ticket_addr(&self.endpoint)
    }

    /// Payload this provider sent of `hash`, for a test that checks what a request moved.
    #[cfg(test)]
    pub fn served_bytes(&self, hash: Hash) -> u64 {
        self._policy.served(hash)
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
                // what the first one was missing, so this process never sees the whole blob.
                Some(()) = self.complete.recv() => return Ok(ServeEnd::Downloaded),
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
