//! The receiving endpoint: a fresh in-memory key, no home lock and no store, so a receive runs
//! beside a send from the same home.

use std::time::Duration;

use iroh::{
    Endpoint, EndpointAddr, PublicKey, RelayUrl, TransportAddr, endpoint::Connection,
    protocol::Router,
};
use tokio::sync::mpsc::UnboundedSender;

use super::{ALPN, Congestion, Event, RelayModeChoice, SluiceProtocol, endpoint, events, now_ms};
use crate::{Error, ticket::Body};

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
        let conn = self.endpoint.connect(addr, alpn).await.map_err(|e| {
            Error::Network(format!("cannot reach the provider: {}", crate::plain(&e)))
        })?;
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
