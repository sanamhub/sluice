//! Using a rendezvous server: waiting for it to report this endpoint's public address, and trusting
//! its certificate when it is self-signed.

use std::{
    collections::BTreeSet,
    path::Path,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use iroh::{Endpoint, Watcher, tls::CaTlsConfig, unstable_net_report::NetReport};
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
    let started = Instant::now();
    let mut learned = BTreeSet::new();
    let mut complete = true;
    for endpoint in endpoints {
        let left = DEADLINE.saturating_sub(started.elapsed());
        // On a timeout the wait is dropped, and the report is read once more for what it holds now.
        let _ = tokio::time::timeout(left, first_report(endpoint)).await;
        let found = reported(endpoint.net_report().get().as_ref());
        complete &= !found.is_empty();
        learned.extend(found);
    }
    Discovery {
        took_s: started.elapsed().as_secs_f64(),
        complete,
        learned: learned.into_iter().collect(),
    }
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
fn reported(report: Option<&NetReport>) -> Vec<String> {
    let Some(report) = report else {
        return Vec::new();
    };
    let v4 = report.global_v4.map(|a| a.to_string());
    let v6 = report.global_v6.map(|a| a.to_string());
    v4.into_iter().chain(v6).collect()
}
