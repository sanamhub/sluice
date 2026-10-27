//! Binding a provider: its endpoint, its access policy and the protocols it accepts. What a send
//! waits for after that is in the parent module.

use std::{
    collections::BTreeSet,
    ops::Deref,
    sync::{Arc, Mutex},
};

use iroh::{Endpoint, protocol::Router};
use iroh_blobs::{BlobsProtocol, Hash, store::fs::FsStore};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::Provider;
use crate::engine::{
    ALPN, Congestion, Rate, RelayModeChoice, SluiceProtocol, endpoint, live::LiveLog, policy,
};
use crate::{Error, Home};

/// Binds the provider's endpoint, with the identity key of `home`, and the completion signal a
/// one-download stop watches. A named port that cannot be bound is an error the user can act on; a
/// saved port that is taken falls back to any port, because a ticket is better than a refusal.
async fn open_endpoint(
    home: &Home,
    relay: &RelayModeChoice,
    congestion: Congestion,
    port: Option<u16>,
    complete: UnboundedSender<u64>,
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
}
