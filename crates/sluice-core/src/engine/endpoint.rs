//! Endpoint construction. Every setting that reaches the network is chosen here, so `send` and
//! `recv` cannot drift apart.
//!
//! The defaults follow ADR 0011: no relay and no n0 discovery unless the user named one. mDNS and
//! port mapping are on, because neither leaves the local network for a service we do not run.

use std::{str::FromStr, sync::Arc, time::Duration};

use iroh::{
    Endpoint, NET_REPORT_TIMEOUT, RelayMap, RelayMode, SecretKey, Watcher,
    endpoint::{
        AfterHandshakeOutcome, Connection, EndpointHooks, PortmapperConfig, QuicTransportConfig,
        presets,
    },
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use noq::congestion::{Bbr3Config, CubicConfig};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;

use super::conn::ConnIds;
use crate::Error;

/// The warning printed when `--n0-relays` sends traffic to n0's public services.
pub const N0_WARNING: &str = "warning: --n0-relays uses n0's public relays and DNS discovery, which this project does not run";

/// The congestion controller of a transfer. Cubic is the default until a noq release carries the
/// BBRv3 maximum-bandwidth fix (ADR 0023); BBRv3 stays selectable as ADR 0015 promised.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Congestion {
    /// The default, and the one every release so far was measured with.
    #[default]
    Cubic,
    /// Experimental BBRv3.
    Bbr3,
}

impl Congestion {
    /// The name used on the command line and in an event.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cubic => "cubic",
            Self::Bbr3 => "bbr3",
        }
    }

    /// The transport config that selects this controller. Windows stay at noq's defaults on
    /// purpose: the bandwidth-delay sizing and several connections wait for the transport
    /// measurement (ADR 0016).
    fn transport(self) -> QuicTransportConfig {
        let factory: Arc<dyn noq::congestion::ControllerFactory + Send + Sync> = match self {
            Self::Cubic => Arc::new(CubicConfig::default()),
            Self::Bbr3 => Arc::new(Bbr3Config::default()),
        };
        QuicTransportConfig::builder()
            .congestion_controller_factory(factory)
            .build()
    }
}

impl std::fmt::Display for Congestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// How the endpoint reaches peers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RelayModeChoice {
    /// No relay. LAN, direct internet paths and port mapping still work.
    #[default]
    Disabled,
    /// The relay URL the ticket names or the user gave.
    Url(String),
    /// n0's public relays and DNS discovery, only with `--n0-relays`.
    N0,
}

impl RelayModeChoice {
    /// The choice from the two flags. A named relay and `--n0-relays` together are refused,
    /// because one endpoint has one relay mode and silently dropping either would surprise.
    pub fn from_flags(relay: Option<&str>, n0_relays: bool) -> Result<Self, Error> {
        match (relay, n0_relays) {
            (Some(_), true) => Err(Error::Policy(
                "--relay and --n0-relays ask for two different relay sets; give one".to_string(),
            )),
            (Some(url), false) => Ok(Self::Url(url.to_string())),
            (None, true) => Ok(Self::N0),
            (None, false) => Ok(Self::Disabled),
        }
    }

    fn iroh(&self) -> Result<RelayMode, Error> {
        match self {
            Self::Disabled => Ok(RelayMode::Disabled),
            Self::N0 => Ok(RelayMode::Default),
            Self::Url(url) => {
                let url = iroh::RelayUrl::from_str(url).map_err(|_| {
                    Error::Policy(format!(
                        "{url:?} is not a relay URL; give an absolute https or http URL"
                    ))
                })?;
                Ok(RelayMode::Custom(RelayMap::from(url)))
            }
        }
    }
}

/// Binds an endpoint, with the identity key `secret` when one is given and a fresh in-memory key
/// otherwise.
pub async fn bind(
    secret: Option<[u8; 32]>,
    relay: &RelayModeChoice,
    congestion: Congestion,
    port: Option<u16>,
) -> Result<Endpoint, Error> {
    bind_inner(secret, relay, congestion, port, None).await
}

/// [`bind`] for a provider whose one-download stop waits for the receiver's completion code.
/// `complete` receives the connection that closed with it, which the provider checks against the
/// roots that connection asked for. `ids` gives each connection the id that check is keyed by, and
/// is the same map the policy reads.
pub(super) async fn bind_with_completion(
    secret: Option<[u8; 32]>,
    relay: &RelayModeChoice,
    congestion: Congestion,
    port: Option<u16>,
    complete: Completion,
) -> Result<Endpoint, Error> {
    bind_inner(secret, relay, congestion, port, Some(complete)).await
}

/// How long a closed connection's id stays the answer for its address, so a request event the
/// provider reports after the close is still credited to the connection that made it. An event
/// queued before a close is processed within milliseconds; a quarter of a second is a wide margin,
/// and the address cannot be reused meanwhile because the close task holds the connection.
const LINGER: Duration = Duration::from_millis(250);
/// What a content connection reported: that a receiver opened it, or that it closed and whether the
/// reason was the completion code. The endpoint id is the peer's, so a receiver that reconnects is
/// the same receiver; the connection id is the one the policy knows, so the provider can check what
/// that connection asked for before it believes a completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Receiver {
    /// A receiver's content connection is up.
    Opened {
        connection: u64,
        endpoint: [u8; 32],
        relayed: bool,
    },
    /// A receiver's content connection closed. `claimed_whole` is only what the peer said; it is
    /// believed only by a provider that saw the connection ask for every root.
    Closed {
        connection: u64,
        endpoint: [u8; 32],
        relayed: bool,
        claimed_whole: bool,
    },
}

/// The completion watch of a provider: every content connection that opens and closes, and the ids
/// the policy knows connections by.
#[derive(Clone, Debug)]
pub(super) struct Completion {
    pub sender: UnboundedSender<Receiver>,
    pub ids: Arc<ConnIds>,
}

/// A watcher that reports every content connection, and whether it closed with
/// [`super::COMPLETE_CLOSE`]. Only the static content protocol carries the code, so a `sluice/1`
/// connection that closes with it means nothing here.
#[derive(Debug)]
struct CompletionSignal(Completion);

impl EndpointHooks for CompletionSignal {
    async fn after_handshake<'a>(&'a self, conn: &'a Connection) -> AfterHandshakeOutcome {
        // Only the content protocol reaches the policy, so only its connections need an id, and
        // only they are given one: the map stays the size of the content connections that are
        // alive, and a handshake cannot grow it without limit.
        if conn.alpn() == iroh_blobs::ALPN {
            let stable = conn.stable_id() as u64;
            // Before this connection can send a request, so the policy never has to guess its id
            // from iroh's reusable address.
            let own = self.0.ids.assign(stable);
            let endpoint = *conn.remote_id().as_bytes();
            let _ = self.0.sender.send(Receiver::Opened {
                connection: own,
                endpoint,
                relayed: relayed(conn),
            });
            // `stable_id` is the address of an allocation that is handed to a later connection once
            // this one is freed, and the policy resolves a request event by that address. Holding a
            // handle of this connection keeps the address out of another connection's hands until
            // the id is forgotten, so an event still queued for this one cannot be credited to the
            // connection that follows it.
            let held = conn.clone();
            let reports = self.0.clone();
            n0_future::task::spawn(async move {
                let closed = held.closed().await;
                let claimed_whole = matches!(
                    &closed,
                    iroh::endpoint::ConnectionError::ApplicationClosed(application)
                        if application.error_code.into_inner() == u64::from(super::COMPLETE_CLOSE)
                );
                let _ = reports.sender.send(Receiver::Closed {
                    connection: own,
                    endpoint,
                    relayed: relayed(&held),
                    claimed_whole,
                });
                // The provider reports a close from its own task while the request it read just
                // before the close is reported by another, so the close can be processed first.
                // The id stays the address's answer for a moment, which gives that event the
                // connection that made it instead of the one that inherits the address.
                tokio::time::sleep(LINGER).await;
                reports.ids.closed(stable, own);
                drop(held);
            });
        }
        AfterHandshakeOutcome::accept()
    }
}

/// Whether the connection reaches its peer through a relay: no path of it is a direct one.
fn relayed(conn: &Connection) -> bool {
    let paths = conn.paths();
    !paths.iter().any(|path| !path.is_relay())
}

async fn bind_inner(
    secret: Option<[u8; 32]>,
    relay: &RelayModeChoice,
    congestion: Congestion,
    port: Option<u16>,
    complete: Option<Completion>,
) -> Result<Endpoint, Error> {
    let mut builder = Endpoint::builder(presets::Empty)
        .crypto_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .relay_mode(relay.iroh()?)
        .portmapper_config(PortmapperConfig::default())
        .transport_config(congestion.transport())
        .address_lookup(MdnsAddressLookup::builder());
    if let Some(complete) = complete {
        builder = builder.hooks(CompletionSignal(complete));
    }
    if let Some(seed) = secret {
        builder = builder.secret_key(SecretKey::from_bytes(&seed));
    }
    if matches!(relay, RelayModeChoice::N0) {
        use iroh::address_lookup::{DnsAddressLookup, PkarrPublisher, PkarrResolver};
        builder = builder
            .address_lookup(PkarrPublisher::n0_dns())
            .address_lookup(PkarrResolver::n0_dns())
            .address_lookup(DnsAddressLookup::n0_dns());
    }
    // A saved port is tried first so a ticket printed by the last send still dials this one. If it
    // is taken, the caller retries with `None` and any port.
    let builder = match port {
        Some(port) => builder
            .bind_addr((std::net::Ipv4Addr::UNSPECIFIED, port))
            .map_err(|e| Error::Network(format!("the saved port {port} cannot be used: {e}")))?,
        None => builder,
    };
    builder.bind().await.map_err(|e| {
        Error::Network(format!(
            "cannot open the local UDP socket for a transfer: {e}"
        ))
    })
}

/// Waits until the endpoint knows at least one direct address. A ticket printed before that cannot
/// be dialled without a relay, and a relay may not exist.
pub async fn wait_for_direct_addr(endpoint: &Endpoint) -> Result<(), Error> {
    let wait = Duration::from_secs(NET_REPORT_TIMEOUT);
    let found = async {
        let mut watcher = endpoint.watch_addr();
        let mut addr = watcher.get();
        while addr.ip_addrs().next().is_none() {
            addr = watcher.updated().await.map_err(|_| {
                Error::Network("the endpoint closed before it had an address".into())
            })?;
        }
        Ok(())
    };
    tokio::time::timeout(wait, found).await.map_err(|_| {
        Error::Network(format!(
            "no direct address appeared within {} s; is a network interface up?",
            wait.as_secs()
        ))
    })?
}

/// Waits until the endpoint has contacted a relay, so its home relay is in the ticket.
pub async fn wait_for_online(endpoint: &Endpoint) -> Result<(), Error> {
    let wait = Duration::from_secs(NET_REPORT_TIMEOUT);
    tokio::time::timeout(wait, endpoint.online())
        .await
        .map_err(|_| Error::Network("the relay did not answer in time".to_string()))?;
    Ok(())
}

/// The address to put in a ticket: the endpoint id, its direct addresses and, when one is set, its
/// relay. iroh's own `addr` includes the home relay the endpoint learned about.
pub fn ticket_addr(endpoint: &Endpoint) -> iroh::EndpointAddr {
    endpoint.addr()
}

#[cfg(test)]
mod tests {
    use super::RelayModeChoice;
    use crate::Error;

    #[test]
    fn a_relay_and_n0_together_are_refused() {
        let err = RelayModeChoice::from_flags(Some("https://relay.example/"), true).unwrap_err();
        assert!(matches!(err, Error::Policy(_)));
    }

    #[test]
    fn no_flag_means_no_relay() {
        assert_eq!(
            RelayModeChoice::from_flags(None, false).unwrap(),
            RelayModeChoice::Disabled
        );
    }

    #[test]
    fn a_bad_url_names_the_form_not_the_value() {
        let err = RelayModeChoice::from_flags(Some("relay"), false)
            .unwrap()
            .iroh()
            .unwrap_err();
        assert!(err.to_string().contains("absolute https or http URL"));
    }
}
