//! Binding a provider: its endpoint, its access policy and the protocols it accepts. What a send
//! waits for after that is in the parent module.

use std::{
    collections::BTreeSet,
    ops::Deref,
    sync::{Arc, Mutex},
};

use iroh::{Endpoint, protocol::Router};
use iroh_blobs::{BlobsProtocol, Hash, store::fs::FsStore};
use tokio::sync::mpsc::unbounded_channel;

use super::Provider;
use crate::engine::{
    ALPN, Congestion, Rate, RelayModeChoice, SluiceProtocol, conn::ConnIds, endpoint,
    live::LiveLog, policy,
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
    complete: endpoint::Completion,
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
        let (receivers, receivers_rx) = unbounded_channel();
        let own_ids = Arc::new(ConnIds::default());
        let places = Arc::new(Mutex::new(policy::Places::default()));
        // A send in a process that started the daemon's listener installs its protocols there, so
        // the daemon has one endpoint id whether or not a send is running. That endpoint's own
        // handshake ids and connection reports are shared with the send, so a limited send still
        // knows which receiver took which place. Every other send binds an endpoint of its own.
        let (endpoint, attached, ids) =
            match crate::engine::paired::attach_for(home, receivers.clone()).await? {
                Some(attached) => {
                    let ids = attached.ids();
                    (attached.endpoint().clone(), Some(attached), ids)
                }
                None => {
                    let completion = endpoint::Completion {
                        sender: receivers.clone(),
                        ids: Arc::clone(&own_ids),
                    };
                    let endpoint =
                        open_endpoint(home, &relay, congestion, port, completion).await?;
                    if let Some(port) = endpoint
                        .bound_sockets()
                        .first()
                        .map(std::net::SocketAddr::port)
                    {
                        home.save_port(port);
                    }
                    if !matches!(relay, RelayModeChoice::Disabled | RelayModeChoice::Loopback) {
                        endpoint::wait_for_online(&endpoint).await?;
                    }
                    endpoint::wait_for_direct_addr(&endpoint).await?;
                    (endpoint, None, own_ids)
                }
            };
        let (done, done_rx) = unbounded_channel();
        let allowed_for_delta = Arc::clone(&allowed);
        let policy = policy::start_shared(
            allowed,
            expiry,
            done,
            store.clone(),
            rate,
            ids,
            Arc::clone(&places),
            receivers,
        );
        let blobs = BlobsProtocol::new(store.deref(), Some(policy.events.clone()));
        let receipts =
            crate::engine::receipts::Receipts::new(policy.asked_handle(), Arc::clone(&places));
        // A live stream has no root to sign chunks of, so only a fixed send serves signatures.
        let delta = live.is_none().then(|| {
            crate::engine::delta::Delta::new(
                store.clone(),
                allowed_for_delta,
                policy.asked_handle(),
                Arc::clone(&places),
            )
        });
        let sluice = SluiceProtocol::serving(live, Arc::clone(&receipts), delta.clone());
        let router = match &attached {
            Some(attached) => {
                attached.install(blobs, sluice)?;
                None
            }
            None => Some(
                Router::builder(endpoint.clone())
                    .accept(iroh_blobs::ALPN, blobs)
                    .accept(ALPN, sluice)
                    .spawn(),
            ),
        };
        Ok(Self {
            router,
            attached,
            endpoint,
            store,
            policy,
            receipts,
            done: done_rx,
            receivers: receivers_rx,
            places,
            group_lines: Mutex::new(policy::Lines::default()),
            delta,
            expiry,
            _dir: None,
        })
    }
}
