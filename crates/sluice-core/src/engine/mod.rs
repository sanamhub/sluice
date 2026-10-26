//! The session layer: endpoints, the provider's access policy, the `sluice/1` greeting and the
//! events a transfer reports. Commands start a runtime; this module only provides futures.

mod endpoint;
mod events;
mod hello;
mod policy;

use std::{
    collections::BTreeSet,
    ops::Deref,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayUrl, TransportAddr, endpoint::Connection,
    protocol::Router,
};
use iroh_blobs::{BlobsProtocol, Hash, store::fs::FsStore};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::{Error, Home, ticket::Body};

pub use endpoint::{N0_WARNING, RelayModeChoice};
pub use events::{Event, PathKind, emit, watch_path};
pub use hello::{
    ALPN, CAP_DELTA, CAP_LIVE, CAP_MANIFEST_V1, CAP_PACKS, CAP_RECEIPTS, CAP_ZSTD, CAPABILITIES,
    Hello, MAX_FRAME_BYTES, PROTO_VERSION, SluiceProtocol,
};

/// When `send` stops serving.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// After one request for this hash completes, which is the file the receiver asked for.
    OneDownload([u8; 32]),
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
    done: tokio::sync::mpsc::UnboundedReceiver<Hash>,
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
    /// access policy for every request type.
    pub async fn start(
        home: &Home,
        store: FsStore,
        allowed: BTreeSet<[u8; 32]>,
        relay: RelayModeChoice,
        expiry: Option<u64>,
    ) -> Result<Self, Error> {
        let seed = home.identity()?.secret_seed();
        let endpoint = endpoint::bind(Some(seed), &relay).await?;
        endpoint::wait_for_direct_addr(&endpoint).await?;
        let allowed: BTreeSet<Hash> = allowed.into_iter().map(Hash::from).collect();
        let (done, done_rx) = unbounded_channel();
        let policy = policy::start(allowed, expiry, done);
        let blobs = BlobsProtocol::new(store.deref(), Some(policy.events.clone()));
        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(ALPN, SluiceProtocol)
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

    /// The store, for an import before or a check after serving.
    pub fn store(&self) -> &FsStore {
        &self.store
    }

    /// Serves until the requested stop. The router is shut down before this returns, so a closed
    /// provider is not still accepting connections.
    pub async fn serve(mut self, stop: Stop) -> Result<ServeEnd, Error> {
        let end = match stop {
            Stop::OneDownload(root) => self.one_download(Hash::from(root)).await?,
            Stop::Keep => self.keep().await?,
        };
        self.shutdown_router().await?;
        Ok(end)
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

    async fn one_download(&mut self, root: Hash) -> Result<ServeEnd, Error> {
        let wait = match self.expiry {
            Some(expiry) => expiry.saturating_sub(now_ms()),
            None => 100 * 365 * 24 * 60 * 60 * 1000,
        };
        if wait == 0 {
            return Ok(ServeEnd::Expired);
        }
        let deadline = tokio::time::sleep(Duration::from_millis(wait));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                done = self.done.recv() => match done {
                    Some(hash) if hash == root => return Ok(ServeEnd::Downloaded),
                    Some(_) => continue,
                    None => return Err(Error::Bug("the provider event task stopped".into())),
                },
                _ = &mut deadline => return Ok(ServeEnd::Expired),
            }
        }
    }

    async fn keep(&mut self) -> Result<ServeEnd, Error> {
        let wait = match self.expiry {
            Some(expiry) => Duration::from_millis(expiry.saturating_sub(now_ms())),
            None => Duration::from_secs(100 * 365 * 24 * 60 * 60),
        };
        let deadline = tokio::time::sleep(wait);
        tokio::pin!(deadline);
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Ok(ServeEnd::Interrupted),
            _ = &mut deadline => Ok(ServeEnd::Expired),
        }
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
    pub async fn open(relay: RelayModeChoice) -> Result<Self, Error> {
        let endpoint = endpoint::bind(None, &relay).await?;
        if !matches!(relay, RelayModeChoice::Disabled) {
            let wait = Duration::from_secs(iroh::NET_REPORT_TIMEOUT);
            tokio::time::timeout(wait, endpoint.online())
                .await
                .map_err(|_| Error::Network("the relay did not answer in time".to_string()))?;
        }
        let router = Router::builder(endpoint.clone())
            .accept(ALPN, SluiceProtocol)
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
        let alpn = match ticket.body() {
            Body::Static { .. } => iroh_blobs::ALPN,
            Body::Live { .. } => ALPN,
        };
        let conn = self
            .endpoint
            .connect(addr, alpn)
            .await
            .map_err(|e| Error::Network(format!("cannot reach the provider: {e}")))?;
        let _watch = events::watch_path(conn.clone(), events);
        Ok(conn)
    }

    /// Closes the endpoint cleanly.
    pub async fn shutdown(mut self) -> Result<(), Error> {
        if let Some(router) = self.router.take() {
            router
                .shutdown()
                .await
                .map_err(|e| Error::Network(format!("cannot close the receiver: {e}")))?;
        }
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[cfg(test)]
mod tests;
