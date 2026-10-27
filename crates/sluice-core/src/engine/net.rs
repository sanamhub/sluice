//! What the network looks like from this machine, for `sluice doctor --network`. It asks a relay
//! for a net report and says what came back; nothing here changes a setting or keeps a socket.

use std::net::SocketAddr;

use iroh::Watcher;

use super::{Congestion, RelayModeChoice, endpoint};
use crate::Error;

/// What a relay saw of this machine. A check reports it and never fails on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkReport {
    /// A round trip over UDP with an IPv4 address completed.
    pub udp_v4: bool,
    /// A round trip over UDP with an IPv6 address completed.
    pub udp_v6: bool,
    /// The public IPv4 address the relay saw.
    pub global_v4: Option<SocketAddr>,
    /// The public IPv6 address the relay saw.
    pub global_v6: Option<SocketAddr>,
    /// Whether two servers saw different public addresses, which is a hard NAT. `None` when only
    /// one server answered.
    pub mapping_varies: Option<bool>,
    /// The relay with the lowest measured latency.
    pub preferred_relay: Option<String>,
}

impl NetworkReport {
    /// Whether any UDP round trip worked, which is what a direct path needs.
    pub fn has_udp(&self) -> bool {
        self.udp_v4 || self.udp_v6
    }
}

/// Asks the relays of `relay` for a net report. It binds a fresh endpoint and closes it before
/// returning, so `doctor` leaves no socket behind. A named relay that cannot be reached is an
/// [`Error::Network`], which the caller reports as a warning.
pub async fn report(relay: &RelayModeChoice) -> Result<NetworkReport, Error> {
    let endpoint = endpoint::bind(None, relay, Congestion::Cubic, None).await?;
    let outcome = async {
        endpoint::wait_for_online(&endpoint).await?;
        let mut watcher = endpoint.net_report();
        let report = watcher
            .get()
            .ok_or_else(|| Error::Network("the relay sent no net report".to_string()))?;
        Ok(NetworkReport {
            udp_v4: report.udp_v4,
            udp_v6: report.udp_v6,
            global_v4: report.global_v4.map(SocketAddr::V4),
            global_v6: report.global_v6.map(SocketAddr::V6),
            mapping_varies: report.mapping_varies_by_dest(),
            preferred_relay: report.preferred_relay.map(|relay| relay.to_string()),
        })
    }
    .await;
    endpoint.close().await;
    outcome
}

#[cfg(test)]
mod tests {
    use super::NetworkReport;
    use std::net::SocketAddr;

    #[test]
    fn a_report_says_whether_a_socket_could_be_reached() {
        let report = NetworkReport {
            udp_v4: true,
            udp_v6: false,
            global_v4: Some("192.0.2.1:4000".parse::<SocketAddr>().unwrap()),
            global_v6: None,
            mapping_varies: Some(true),
            preferred_relay: None,
        };
        assert!(report.has_udp());
        assert_eq!(report.mapping_varies, Some(true));
    }
}
