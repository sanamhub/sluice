//! Dialling the addresses in the ticket, and what it means when no connection comes up.

use std::{
    fmt,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use iroh::{Endpoint, EndpointAddr, endpoint::Connection};
use tokio::task::JoinSet;

use crate::{
    cli::{FetchArgs, Mode, Relay},
    net,
    rendezvous::Discovery,
    result::{Outcome, Output},
    ticket::Ticket,
};

/// No connection came up: the dial failed, or `--timeout` ran out first. It is the result when no
/// relay is there to carry the traffic, so `fetch` reports it and does not fail with it.
#[derive(Debug)]
pub struct DialFailed {
    /// How long the dial ran.
    pub waited: Duration,
    /// What iroh said, or that the time ran out.
    pub error: String,
}

impl fmt::Display for DialFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no connection after {:.1} s: {}",
            self.waited.as_secs_f64(),
            self.error
        )
    }
}

impl std::error::Error for DialFailed {}

/// The connections, and how long the dial took.
pub struct Dialed {
    /// In the order they were asked for.
    pub conns: Vec<Connection>,
    /// From starting the dials to the last connection being up.
    pub took: Duration,
}

/// Opens all connections at once and checks each one's key exchange. Connection `i` goes out
/// of endpoint `i % endpoints` to address `i % addrs`, and the result is in `i` order, so the
/// per-connection statistics line up with the endpoints. Fails with [`DialFailed`] if a dial
/// fails or `limit` passes first. A weak key exchange is a plain error, because it is a bug and
/// not an answer.
pub async fn connect_all(
    endpoints: &[Endpoint],
    addrs: &[EndpointAddr],
    alpn: &'static [u8],
    count: u16,
    limit: Duration,
) -> Result<Dialed> {
    ensure!(!addrs.is_empty(), "the ticket carries no address");
    let started = Instant::now();
    let dial = async {
        let mut dials = JoinSet::new();
        for i in 0..usize::from(count) {
            let endpoint = endpoints[i % endpoints.len()].clone();
            let addr = addrs[i % addrs.len()].clone();
            dials.spawn(async move { endpoint.connect(addr, alpn).await.map(|conn| (i, conn)) });
        }
        let mut conns = Vec::with_capacity(usize::from(count));
        while let Some(joined) = dials.join_next().await {
            let dialled = joined.context("dial task panicked")?;
            let (i, conn) = dialled.map_err(|e| DialFailed {
                waited: started.elapsed(),
                error: e.to_string(),
            })?;
            net::require_post_quantum(&conn)?;
            conns.push((i, conn));
        }
        Ok::<_, anyhow::Error>(conns)
    };
    let mut conns = match tokio::time::timeout(limit, dial).await {
        Ok(conns) => conns?,
        Err(_) => {
            return Err(DialFailed {
                waited: limit,
                error: format!("no connection within {} s", limit.as_secs()),
            }
            .into());
        }
    };
    conns.sort_by_key(|(i, _)| *i);
    Ok(Dialed {
        conns: conns.into_iter().map(|(_, conn)| conn).collect(),
        took: started.elapsed(),
    })
}

/// The result line for a dial that failed: nothing was received, and nothing else was measured.
pub fn no_path(
    args: &FetchArgs,
    ticket: &Ticket,
    fetch_endpoints: usize,
    discovery: Option<Discovery>,
    failed: &DialFailed,
) -> Output {
    Output {
        outcome: Outcome::NoPath,
        mode: ticket.mode,
        size_bytes: ticket.size,
        received_bytes: 0,
        connections: args.connections,
        serve_endpoints: ticket.addrs.len(),
        fetch_endpoints,
        serve_store: ticket.store,
        congestion: args.shared.congestion,
        window: net::Windows::for_mode(args.shared.window),
        sink: (ticket.mode == Mode::Blobs).then_some(args.sink),
        relay: match &args.shared.relay {
            Relay::Disabled => "disabled".to_string(),
            Relay::Url(url) => url.to_string(),
        },
        rendezvous: args.shared.rendezvous.as_ref().map(|rv| rv.url.to_string()),
        discovery,
        connect_s: None,
        error: Some(failed.error.clone()),
        elapsed_s: failed.waited.as_secs_f64(),
        throughput_gbit_s: 0.0,
        progress: Vec::new(),
        connection_stats: Vec::new(),
        path: None,
        rtt_ms: None,
        cpu_user_s: None,
        cpu_sys_s: None,
        key_exchange_group: None,
        gso_segments: None,
        gro_segments: None,
        recv_datagrams: 0,
        recv_gro_batches: 0,
        iroh_version: env!("PROBE_IROH_VERSION"),
        iroh_blobs_version: env!("PROBE_IROH_BLOBS_VERSION"),
        verified_hash: None,
    }
}
