//! Using a rendezvous server: waiting for it to report this endpoint's public address, and trusting
//! its certificate when it is self-signed.

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use iroh::{
    Endpoint, EndpointAddr, TransportAddr, Watcher, tls::CaTlsConfig,
    unstable_net_report::NetReport,
};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use serde::{Deserialize, Serialize};

/// Longest the endpoints of one process wait for the server's answer before the probe goes on
/// without it.
pub const DEADLINE: Duration = Duration::from_secs(10);

/// What the rendezvous server told the endpoints of one process.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Discovery {
    /// Seconds from asking to the last answer, or to giving up.
    pub took_s: f64,
    /// Every endpoint had an address from the server before the deadline.
    pub complete: bool,
    /// The public `ip:port` addresses the server reported, once each.
    pub learned: Vec<String>,
    /// Reported addresses `serve` put into the ticket itself, because the endpoint's own address
    /// still lacked them when the deadline ran out. Empty when iroh merged them, and always empty
    /// for `fetch`, which prints no ticket.
    #[serde(default)]
    pub added_from_report: Vec<String>,
}

/// The default roots plus the certificates in the PEM file `path`.
pub fn trust(path: &Path) -> Result<CaTlsConfig> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(path)
        .with_context(|| format!("open {}", path.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("read certificates from {}", path.display()))?;
    ensure!(!certs.is_empty(), "{} holds no certificate", path.display());
    Ok(CaTlsConfig::default().with_extra_roots(certs))
}

/// Waits for every endpoint to have an address from the server, at most [`DEADLINE`] in all. The
/// endpoints ask in parallel on their own, so waiting for them one after another loses nothing.
pub async fn discover(endpoints: &[Endpoint]) -> Discovery {
    discover_from(endpoints, Instant::now()).await
}

/// [`discover`] with the clock started at `started`, for a caller that needs the same deadline
/// for what it does next.
pub async fn discover_from(endpoints: &[Endpoint], started: Instant) -> Discovery {
    let mut learned = BTreeSet::new();
    let mut complete = true;
    for endpoint in endpoints {
        let left = DEADLINE.saturating_sub(started.elapsed());
        // On a timeout the wait is dropped, and the report is read once more for what it holds now.
        let _ = tokio::time::timeout(left, first_report(endpoint)).await;
        let found = reported(endpoint.net_report().get().as_ref());
        complete &= !found.is_empty();
        learned.extend(found.iter().map(ToString::to_string));
    }
    Discovery {
        took_s: started.elapsed().as_secs_f64(),
        complete,
        learned: learned.into_iter().collect(),
        added_from_report: Vec::new(),
    }
}

/// What the server told this endpoint, as the endpoint's report holds it now.
pub fn reported_by(endpoint: &Endpoint) -> Vec<SocketAddr> {
    reported(endpoint.net_report().get().as_ref())
}

/// The endpoint's own address once it holds every address the server reported to it, and the
/// reported addresses it had to be given because it still lacked them at `deadline`.
///
/// The report and the endpoint's address are set by different tasks in iroh 1.3.0: the report
/// at `socket.rs:819`, the address when the socket actor, woken by its own watcher on the report
/// (`socket.rs:1550`), reaches `store_direct_addresses` (`socket.rs:1886`). A caller woken by the
/// same report can read the address first. On a NAT the missing address is the public one, and a
/// ticket without it sends `fetch` to private addresses only.
pub async fn merged_addr(endpoint: &Endpoint, deadline: Instant) -> (EndpointAddr, Vec<String>) {
    let wanted = reported_by(endpoint);
    let mut watcher = endpoint.watch_addr();
    let merged = async {
        let mut addr = watcher.get();
        while !missing(&addr, &wanted).is_empty() {
            // The endpoint closed, so no merge is coming.
            addr = watcher.updated().await.ok()?;
        }
        Some(addr)
    };
    let deadline = tokio::time::Instant::from_std(deadline);
    let _ = tokio::time::timeout_at(deadline, merged).await;
    add_missing(watcher.get(), &wanted)
}

/// The addresses in `wanted` that `addr` does not hold.
fn missing(addr: &EndpointAddr, wanted: &[SocketAddr]) -> Vec<SocketAddr> {
    let held: BTreeSet<&SocketAddr> = addr.ip_addrs().collect();
    wanted
        .iter()
        .filter(|w| !held.contains(w))
        .copied()
        .collect()
}

/// `addr` with every address in `wanted` that it lacked, and those addresses.
fn add_missing(mut addr: EndpointAddr, wanted: &[SocketAddr]) -> (EndpointAddr, Vec<String>) {
    let absent = missing(&addr, wanted);
    addr.addrs
        .extend(absent.iter().map(|a| TransportAddr::Ip(*a)));
    (addr, absent.iter().map(ToString::to_string).collect())
}

async fn first_report(endpoint: &Endpoint) {
    let mut watcher = endpoint.net_report();
    let mut report = watcher.get();
    while reported(report.as_ref()).is_empty() {
        match watcher.updated().await {
            Ok(next) => report = next,
            // The endpoint closed, so no answer is coming.
            Err(_) => return,
        }
    }
}

/// The addresses a net report holds, which the server told the endpoint.
fn reported(report: Option<&NetReport>) -> Vec<SocketAddr> {
    let Some(report) = report else {
        return Vec::new();
    };
    let v4 = report.global_v4.map(SocketAddr::from);
    let v6 = report.global_v6.map(SocketAddr::from);
    v4.into_iter().chain(v6).collect()
}

#[cfg(test)]
mod tests {
    use iroh::SecretKey;

    use super::*;

    fn addr_with(ips: &[&str]) -> EndpointAddr {
        let id = SecretKey::generate().public();
        EndpointAddr::from_parts(
            id,
            ips.iter()
                .map(|ip| TransportAddr::Ip(ip.parse().expect("socket address"))),
        )
    }

    fn sockets(ips: &[&str]) -> Vec<SocketAddr> {
        ips.iter()
            .map(|ip| ip.parse().expect("socket address"))
            .collect()
    }

    #[test]
    fn a_held_address_is_not_added() {
        let addr = addr_with(&["10.0.0.2:4000", "203.0.113.9:4000"]);
        let (merged, added) = add_missing(addr.clone(), &sockets(&["203.0.113.9:4000"]));
        assert!(added.is_empty());
        assert_eq!(merged, addr);
    }

    #[test]
    fn a_missing_address_is_added_and_named() {
        let addr = addr_with(&["10.0.0.2:4000"]);
        let wanted = sockets(&["203.0.113.9:4000", "10.0.0.2:4000"]);
        let (merged, added) = add_missing(addr, &wanted);
        assert_eq!(added, ["203.0.113.9:4000"]);
        let held: Vec<String> = merged.ip_addrs().map(ToString::to_string).collect();
        assert_eq!(held, ["10.0.0.2:4000", "203.0.113.9:4000"]);
    }

    #[test]
    fn a_port_that_differs_is_a_different_address() {
        let addr = addr_with(&["203.0.113.9:4000"]);
        assert_eq!(missing(&addr, &sockets(&["203.0.113.9:4001"])).len(), 1);
    }
}
