//! Endpoint construction. Every setting that reaches the network is chosen here, so `send` and
//! `recv` cannot drift apart.
//!
//! The defaults follow ADR 0011: no relay and no n0 discovery unless the user named one. mDNS and
//! port mapping are on, because neither leaves the local network for a service we do not run.

use std::{str::FromStr, sync::Arc, time::Duration};

use iroh::{
    Endpoint, NET_REPORT_TIMEOUT, RelayMap, RelayMode, SecretKey, Watcher,
    endpoint::{PortmapperConfig, QuicTransportConfig, presets},
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use noq::congestion::{Bbr3Config, CubicConfig};
use serde::Serialize;

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
) -> Result<Endpoint, Error> {
    let mut builder = Endpoint::builder(presets::Empty)
        .crypto_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .relay_mode(relay.iroh()?)
        .portmapper_config(PortmapperConfig::default())
        .transport_config(congestion.transport())
        .address_lookup(MdnsAddressLookup::builder());
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
