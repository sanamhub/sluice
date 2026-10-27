//! The session layer: endpoints, the provider's access policy, the `sluice/1` greeting and the
//! events a transfer reports. Commands start a runtime; this module only provides futures.

mod endpoint;
mod events;
mod expiry;
mod fetch;
mod hello;
mod live;
mod net;
mod policy;
mod rate;
mod recv;
mod recv_range;
mod recv_stdout;
mod send;
mod storage;

use std::{
    collections::BTreeSet,
    ops::Deref,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayUrl, TransportAddr, endpoint::Connection,
    protocol::Router,
};
use iroh_blobs::{BlobsProtocol, Hash, store::fs::FsStore};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::{Error, Home, ticket::Body};

pub use endpoint::{Congestion, N0_WARNING, RelayModeChoice};
pub use events::{Event, PathKind, emit, watch_path};
pub use expiry::{DEFAULT_SECONDS, deadline, parse as parse_expiry};
pub use hello::{
    ALPN, CAP_DELTA, CAP_LIVE, CAP_MANIFEST_V1, CAP_PACKS, CAP_RECEIPTS, CAP_ZSTD, CAPABILITIES,
    Hello, MAX_FRAME_BYTES, PROTO_VERSION, SluiceProtocol, decode as decode_hello,
    encode as encode_hello, greet,
};
pub use live::{CLOSE_AFTER, LiveLog, LiveSend, prepare_live, receive_live};
pub use net::{NetworkReport, report as network_report};
pub use rate::Rate;
pub use recv::{ReceiveOptions, Received, receive, top_level_rule};
pub use recv_range::receive_range;
pub use recv_stdout::receive_stdout;
pub use send::{
    Outgoing, StopEnd, default_expiry, prepare as prepare_send,
    prepare_paths as prepare_send_paths, prepare_stdin as prepare_send_stdin,
};

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
        Self::start_inner(home, store, allowed, None, relay, congestion, expiry, port).await
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
        let policy = policy::start_shared(allowed, expiry, done, store.clone());
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

    /// Caps the bytes this provider sends per second, shared by every request it serves.
    pub fn set_rate(&self, rate: Option<Arc<Rate>>) {
        self._policy.set_rate(rate);
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

/// The receiving end: an endpoint with a fresh in-memory key, no home lock and no store.
pub struct Receiver {
    router: Option<Router>,
    endpoint: Endpoint,
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receiver")
            .field("endpoint", &self.endpoint.id().fmt_short().to_string())
            .finish_non_exhaustive()
    }
}

impl Receiver {
    /// Binds a receiver. It takes no lock, so it runs beside a `send`.
    pub async fn open(relay: RelayModeChoice, congestion: Congestion) -> Result<Self, Error> {
        let endpoint = endpoint::bind(None, &relay, congestion, None).await?;
        if !matches!(relay, RelayModeChoice::Disabled) {
            let wait = Duration::from_secs(iroh::NET_REPORT_TIMEOUT);
            tokio::time::timeout(wait, endpoint.online())
                .await
                .map_err(|_| Error::Network("the relay did not answer in time".to_string()))?;
        }
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, SluiceProtocol::default())
            .spawn();
        Ok(Self {
            router: Some(router),
            endpoint,
        })
    }

    /// Connects to the provider the ticket names, on the ALPN its kind needs. An expired ticket is
    /// refused before a socket is opened. `events` gets the first path and every change.
    pub async fn connect(
        &self,
        ticket: &crate::ticket::Ticket,
        events: UnboundedSender<Event>,
    ) -> Result<Connection, Error> {
        let alpn = match ticket.body() {
            Body::Static { .. } => iroh_blobs::ALPN,
            Body::Live { .. } => ALPN,
        };
        self.connect_alpn(ticket, alpn, events).await
    }

    /// Connects on the static blob ALPN, which is the second connection a live receive opens for
    /// the segment content.
    pub async fn connect_static(
        &self,
        ticket: &crate::ticket::Ticket,
        events: UnboundedSender<Event>,
    ) -> Result<Connection, Error> {
        self.connect_alpn(ticket, iroh_blobs::ALPN, events).await
    }

    /// Connects to the provider the ticket names on `alpn`. An expired ticket is refused before a
    /// socket is opened.
    pub async fn connect_alpn(
        &self,
        ticket: &crate::ticket::Ticket,
        alpn: &[u8],
        events: UnboundedSender<Event>,
    ) -> Result<Connection, Error> {
        if ticket.is_expired(now_ms()) {
            return Err(Error::Policy(
                "the ticket has expired; ask the sender for a new one".to_string(),
            ));
        }
        let id = PublicKey::from_bytes(ticket.endpoint_id())
            .map_err(|_| Error::Policy("the ticket names an unreadable endpoint".to_string()))?;
        let mut addr = EndpointAddr::new(id);
        for direct in ticket.addrs() {
            addr.addrs.insert(TransportAddr::Ip(*direct));
        }
        if let Some(text) = ticket.relay_url() {
            let relay = text
                .parse::<RelayUrl>()
                .map_err(|_| Error::Policy("the ticket relay URL cannot be read".to_string()))?;
            addr.addrs.insert(TransportAddr::Relay(relay));
        }
        let conn = self
            .endpoint
            .connect(addr, alpn)
            .await
            .map_err(|e| Error::Network(format!("cannot reach the provider: {e}")))?;
        let _watch = events::watch_path(conn.clone(), events);
        Ok(conn)
    }

    /// Closes the endpoint cleanly. It takes `&self` so a caller can close it early and still run
    /// the close again on the way out; shutting down twice is not an error.
    pub async fn shutdown(&self) -> Result<(), Error> {
        if let Some(router) = &self.router {
            router
                .shutdown()
                .await
                .map_err(|e| Error::Network(format!("cannot close the receiver: {e}")))?;
        }
        Ok(())
    }
}

/// The one wall clock a compare reads: UTC milliseconds since the Unix epoch. A ticket's boundary
/// and a sender's deadline are the same instant, so they are read the same way.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
