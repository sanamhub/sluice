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
use crate::{Error, Home};

/// When `send` stops serving.
#[derive(Clone, Debug)]
pub enum Stop {
    /// After every content root has been served and the receiver has closed its connection.
    OneDownload(Vec<[u8; 32]>),
    /// A live stream: serve until its close is signed, then until every segment is delivered.
    Live(Arc<LiveLog>),
    /// On Ctrl+C, or at the ticket expiry.
    Keep,
}

/// Why serving ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServeEnd {
    /// The one download finished.
    Downloaded,
    /// The ticket expired and the provider stopped serving it.
    Expired,
    /// Ctrl+C.
    Interrupted,
}

/// A listening provider. It holds the store open and answers only its allowed hashes.
pub struct Provider {
    router: Option<Router>,
    endpoint: Endpoint,
    store: FsStore,
    _policy: policy::Policy,
    done: tokio::sync::mpsc::UnboundedReceiver<policy::ServeEvent>,
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
        let seed = home.identity()?.secret_seed();
        let saved = home.saved_port().filter(|_| port.is_none());
        let chosen = port.or(saved);
        let endpoint = match endpoint::bind(Some(seed), &relay, congestion, chosen).await {
            Ok(endpoint) => endpoint,
            Err(e) if port.is_some() => {
                let port = port.unwrap_or_default();
                return Err(Error::Storage(format!("cannot bind UDP port {port}: {e}")));
            }
            // The saved port is taken or unusable; any port is better than refusing to send.
            Err(_) if saved.is_some() => {
                endpoint::bind(Some(seed), &relay, congestion, None).await?
            }
            Err(e) => return Err(e),
        };
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

/// The grace period after the last content root is delivered while a receiver keeps its connection
/// open. A test uses a short one so the rule can be watched without waiting half a minute.
const GRACE: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 30 });

/// Waits for the grace period, or for ever when none has started. A plain `Option` cannot be
/// selected on, and a fresh sleep would restart the period on every loop pass.
async fn wait_grace(grace: &mut Option<std::pin::Pin<Box<tokio::time::Sleep>>>) {
    match grace {
        Some(sleep) => sleep.as_mut().await,
        None => std::future::pending::<()>().await,
    }
}
