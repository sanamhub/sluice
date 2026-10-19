//! `probe fetch`: one transfer, one result.

use std::{
    ops::Deref,
    path::PathBuf,
    str::FromStr,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, ensure};
use iroh::{
    Endpoint, EndpointAddr,
    endpoint::{Connection, VarInt},
};
use iroh_blobs::{Hash, store::fs::FsStore};
use rustls::NamedGroup;
use tokio::task::JoinSet;

use crate::{
    blobs,
    cli::{EndpointMode, FetchArgs, Mode, Relay, Sink},
    cpu::{self, Cpu},
    data, net, raw,
    result::{Outcome, Output},
    stats::{self, ConnReport},
    ticket::Ticket,
    transfer::{self, Counters, Plan},
};

/// What was observed between the first request and the last verified byte, or the timeout.
struct Measured {
    outcome: Outcome,
    received: u64,
    elapsed: Duration,
    progress: Vec<u64>,
    connections: Vec<ConnReport>,
    cpu: Option<Cpu>,
    datagrams: u64,
    gro_batches: u64,
    path: Option<&'static str>,
    rtt: Option<Duration>,
    group: NamedGroup,
}

/// A store directory removed on drop, unless the caller named one to keep.
struct Scratch(Option<PathBuf>);

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(dir) = &self.0 {
            // Best effort: a leftover temp directory is not worth failing a finished measurement.
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Runs the transfer described by the ticket and reports it.
pub async fn run(args: FetchArgs) -> Result<Output> {
    let ticket = Ticket::decode(&args.ticket)?;
    ensure!(
        ticket.mode == args.shared.mode,
        "the ticket is for {:?} but --mode is {:?}",
        ticket.mode,
        args.shared.mode
    );
    let verified = match ticket.mode {
        Mode::Raw => None,
        Mode::Blobs => Some(expected_root(&ticket)?),
    };

    let endpoints = bind_endpoints(&args).await?;
    let measured = measure(&endpoints, &args, &ticket, verified).await;
    // Closed on every path, or iroh logs an ungraceful abort over the real error.
    for endpoint in &endpoints {
        endpoint.close().await;
    }
    Ok(output(&args, &ticket, verified, endpoints.len(), measured?))
}

/// One endpoint, or one per connection. Separate endpoints mean separate sockets and socket
/// tasks, which is what ADR 0016's sharding needs if the shared one is the bottleneck.
async fn bind_endpoints(args: &FetchArgs) -> Result<Vec<Endpoint>> {
    let count = match args.endpoints {
        EndpointMode::Shared => 1,
        EndpointMode::Separate => args.connections,
    };
    let mut endpoints = Vec::with_capacity(usize::from(count));
    for _ in 0..count {
        endpoints.push(net::bind(&args.shared).await?);
    }
    Ok(endpoints)
}

async fn measure(
    endpoints: &[Endpoint],
    args: &FetchArgs,
    ticket: &Ticket,
    verified: Option<Hash>,
) -> Result<Measured> {
    let alpn = match ticket.mode {
        Mode::Raw => net::RAW_ALPN,
        Mode::Blobs => iroh_blobs::ALPN,
    };
    let conns = connect_all(endpoints, &ticket.addrs, alpn, args.connections).await?;
    let group = net::require_post_quantum(&conns[0])?;
    tracing::info!(connections = conns.len(), ?group, "connected");

    let scratch = Scratch(match (ticket.mode, args.sink, &args.dir) {
        (Mode::Blobs, Sink::Store, None) => Some(temp_dir()),
        _ => None,
    });
    let fs_store = match (ticket.mode, args.sink) {
        (Mode::Blobs, Sink::Store) => {
            let dir = args
                .dir
                .clone()
                .or_else(|| scratch.0.clone())
                .expect("set above");
            Some(
                FsStore::load(&dir)
                    .await
                    .map_err(|e| anyhow!("open store: {e}"))?,
            )
        }
        _ => None,
    };
    let plan = match verified {
        None => Plan::Raw(raw::split(ticket.size, args.connections)),
        Some(hash) => Plan::Blobs {
            hash,
            parts: blobs::split(ticket.size, args.connections)?,
            store: fs_store.as_ref().map(|s| s.deref().clone()),
        },
    };

    let counters = Counters::new(conns.len());
    let mut progress = Vec::new();
    let cpu_before = cpu::now();
    let (datagrams_before, gro_before) = socket_counters(endpoints);
    let started = Instant::now();
    let limit = Duration::from_secs(args.timeout);
    let ran =
        tokio::time::timeout(limit, transfer::run(&conns, plan, &counters, &mut progress)).await;
    let elapsed = started.elapsed();
    let cpu = cpu::now()
        .zip(cpu_before)
        .map(|(after, before)| after.since(before));
    let (datagrams_after, gro_after) = socket_counters(endpoints);

    // Read before anything closes. A stalled run is only worth having with these in it.
    let counted = counters.per_connection();
    let connections: Vec<ConnReport> = conns
        .iter()
        .zip(&counted)
        .map(|(conn, bytes)| stats::report(conn, Some(*bytes)))
        .collect();
    let (outcome, received, path, rtt) = match ran {
        Ok(received) => {
            let received = received?;
            ensure!(
                received == ticket.size,
                "received {received} payload bytes but the ticket promised {}",
                ticket.size
            );
            if let (Some(fs), Some(hash)) = (&fs_store, verified) {
                let local = fs
                    .remote()
                    .local(hash)
                    .await
                    .map_err(|e| anyhow!("check store: {e}"))?;
                ensure!(
                    local.is_complete(),
                    "the store does not hold the whole blob"
                );
            }
            let (path, rtt) = path_summary(&conns);
            (Outcome::Ok, received, path, rtt)
        }
        Err(_) => {
            let (path, rtt) = path_summary(&conns);
            (Outcome::Timeout, counted.iter().sum(), path, rtt)
        }
    };
    for conn in &conns {
        conn.close(VarInt::from_u32(0), b"done");
    }
    if let Some(fs) = fs_store {
        fs.shutdown()
            .await
            .map_err(|e| anyhow!("close store: {e}"))?;
    }
    drop(scratch);

    Ok(Measured {
        outcome,
        received,
        elapsed,
        progress,
        connections,
        cpu,
        datagrams: datagrams_after - datagrams_before,
        gro_batches: gro_after - gro_before,
        path,
        rtt,
        group,
    })
}

fn output(
    args: &FetchArgs,
    ticket: &Ticket,
    verified: Option<Hash>,
    fetch_endpoints: usize,
    m: Measured,
) -> Output {
    let seconds = m.elapsed.as_secs_f64();
    Output {
        outcome: m.outcome,
        mode: ticket.mode,
        size_bytes: ticket.size,
        received_bytes: m.received,
        connections: args.connections,
        serve_endpoints: ticket.addrs.len(),
        fetch_endpoints,
        serve_store: ticket.store,
        congestion: args.shared.congestion,
        window: net::Windows::for_mode(args.shared.window),
        sink: verified.map(|_| args.sink),
        relay: match &args.shared.relay {
            Relay::Disabled => "disabled".to_string(),
            Relay::Url(url) => url.to_string(),
        },
        elapsed_s: seconds,
        throughput_gbit_s: m.received as f64 * 8.0 / 1e9 / seconds,
        progress: m.progress,
        connection_stats: m.connections,
        path: m.path,
        rtt_ms: m.rtt.map(|rtt| rtt.as_secs_f64() * 1000.0),
        cpu_user_s: m.cpu.map(|c| c.user_s),
        cpu_sys_s: m.cpu.map(|c| c.sys_s),
        key_exchange_group: format!("{:?}", m.group),
        gso_segments: None,
        gro_segments: None,
        recv_datagrams: m.datagrams,
        recv_gro_batches: m.gro_batches,
        iroh_version: env!("PROBE_IROH_VERSION"),
        iroh_blobs_version: env!("PROBE_IROH_BLOBS_VERSION"),
        verified_hash: verified.map(|h| h.to_hex()),
    }
}

/// The root both ends can compute without a data file. A mismatch means the provider imported
/// something other than what the seed and size describe, and nothing measured would mean anything.
fn expected_root(ticket: &Ticket) -> Result<Hash> {
    let expected = Hash::from(data::expected_hash(ticket.seed, ticket.size));
    let advertised = ticket
        .hash
        .as_deref()
        .context("the blobs ticket carries no hash")?;
    let advertised = Hash::from_str(advertised).context("the ticket hash is malformed")?;
    ensure!(
        expected == advertised,
        "the provider imported data that does not match seed {} and size {}",
        ticket.seed,
        ticket.size
    );
    Ok(expected)
}

/// Opens all connections at once and checks each one's key exchange. Connection `i` goes out
/// of endpoint `i % endpoints` to address `i % addrs`, and the result is in `i` order, so the
/// per-connection statistics line up with the endpoints.
async fn connect_all(
    endpoints: &[Endpoint],
    addrs: &[EndpointAddr],
    alpn: &'static [u8],
    count: u16,
) -> Result<Vec<Connection>> {
    ensure!(!addrs.is_empty(), "the ticket carries no address");
    let mut dials = JoinSet::new();
    for i in 0..usize::from(count) {
        let endpoint = endpoints[i % endpoints.len()].clone();
        let addr = addrs[i % addrs.len()].clone();
        dials.spawn(async move {
            let conn = endpoint.connect(addr, alpn).await.context("connect")?;
            Ok::<_, anyhow::Error>((i, conn))
        });
    }
    let mut conns = Vec::with_capacity(usize::from(count));
    while let Some(joined) = dials.join_next().await {
        let (i, conn) = joined.context("dial task panicked")??;
        net::require_post_quantum(&conn)?;
        conns.push((i, conn));
    }
    conns.sort_by_key(|(i, _)| *i);
    Ok(conns.into_iter().map(|(_, conn)| conn).collect())
}

/// `relay` if any connection's selected path goes through a relay, and `direct` if none is, or if
/// no open path is a relay, since then whichever is selected is direct. Otherwise `None`. iroh marks
/// a path selected a little after the first bytes flow, so a short transfer can finish with several
/// paths open and none selected, as it does on a host with two addresses. A connection with exactly
/// one open path has nowhere else to send, so that path counts. The RTT is the first known
/// connection's, `None` if no path is known.
fn path_summary(conns: &[Connection]) -> (Option<&'static str>, Option<Duration>) {
    let (mut relay_in_use, mut relay_open, mut all_known) = (false, false, true);
    let mut first_rtt = None;
    for conn in conns {
        let paths = conn.paths();
        relay_open |= paths.iter().any(|p| p.is_relay());
        let selected = paths.iter().find(|p| p.is_selected());
        match selected.or_else(|| paths.iter().next().filter(|_| paths.len() == 1)) {
            Some(in_use) => {
                relay_in_use |= in_use.is_relay();
                first_rtt.get_or_insert(in_use.rtt());
            }
            None => all_known = false,
        }
    }
    let path = if relay_in_use {
        Some("relay")
    } else if all_known || !relay_open {
        Some("direct")
    } else {
        None
    };
    (path, first_rtt)
}

/// Datagrams received and GRO batches among them, summed over the endpoints, as iroh counts them.
fn socket_counters(endpoints: &[Endpoint]) -> (u64, u64) {
    endpoints.iter().fold((0, 0), |(datagrams, gro), endpoint| {
        let socket = &endpoint.metrics().socket;
        (
            datagrams + socket.recv_datagrams.get(),
            gro + socket.recv_gro_datagrams.get(),
        )
    })
}

fn temp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("sluice-probe-{}-{nanos}", std::process::id()))
}
