//! The native half of the wasm post-quantum spike.
//!
//! It binds an endpoint with the workspace's provider (`aws-lc-rs`), wraps every key exchange group
//! so a completed handshake records which one it was, serves a blob of a size it is told, answers one
//! byte on the spike's ALPN, and writes the three things a page needs: the relay, its endpoint id and
//! the blob's root. Both ends of the interop proof then say the group they agreed on.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use iroh::{
    Endpoint, RelayMap, RelayMode, RelayUrl,
    endpoint::{Connection, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_blobs::{BlobsProtocol, store::mem::MemStore};
use rustls::crypto::CryptoProvider;
use sluice_wasm::pq::{Probe, wrap};

/// The ALPN a handshake probe uses. Nothing but a measurement speaks it.
const SPIKE_ALPN: &[u8] = b"spike/pq/1";

/// How long a bind or a relay registration may take before the process gives up.
const READY_WAIT: Duration = Duration::from_secs(30);

#[derive(Parser, Debug)]
#[command(about = "The native peer of the wasm post-quantum spike")]
struct Args {
    /// The relay both ends meet through.
    #[arg(long, default_value = "http://127.0.0.1:3340/")]
    relay: RelayUrl,
    /// How many bytes of blob to serve.
    #[arg(long, default_value_t = 256 * 1024 * 1024)]
    bytes: usize,
    /// The file the page reads its relay, endpoint id, root and size from.
    #[arg(long, default_value = "config.json")]
    config: std::path::PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let probe = Probe::new();
    let endpoint = Endpoint::builder(presets::Empty)
        .crypto_provider(provider(probe))
        .relay_mode(RelayMode::Custom(RelayMap::from(args.relay.clone())))
        .bind()
        .await
        .context("cannot bind the endpoint")?;
    tokio::time::timeout(READY_WAIT, endpoint.online())
        .await
        .context("the relay did not answer")?;
    let id = endpoint.id();

    let mut content = vec![0u8; args.bytes];
    for (index, byte) in content.iter_mut().enumerate() {
        *byte = u8::try_from(index % 251).expect("a byte");
    }
    let store = MemStore::new();
    let root = store
        .blobs()
        .add_bytes(content)
        .await
        .context("cannot fill the store")?
        .hash;
    let router = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
        .accept(SPIKE_ALPN, Handshake { probe })
        .spawn();

    let config = format!(
        "{{\"relay\":\"{}\",\"endpoint\":\"{id}\",\"root\":\"{root}\",\"size\":{}}}",
        args.relay, args.bytes
    );
    println!("{config}");
    std::fs::write(&args.config, format!("{config}\n"))
        .with_context(|| format!("cannot write {}", args.config.display()))?;

    tokio::signal::ctrl_c().await.context("no signal handler")?;
    router.shutdown().await.context("cannot stop the router")?;
    Ok(())
}

/// The workspace's provider, with every group wrapped so the probe records the one that completes.
fn provider(probe: &'static Probe) -> Arc<CryptoProvider> {
    let aws = rustls::crypto::aws_lc_rs::default_provider();
    let kx_groups = aws
        .kx_groups
        .iter()
        .map(|group| wrap(*group, probe))
        .collect();
    Arc::new(CryptoProvider {
        kx_groups,
        ..aws.clone()
    })
}

/// Answers a probe's connection: says which group the handshake agreed on, reads two bytes and
/// writes them back.
#[derive(Debug)]
struct Handshake {
    probe: &'static Probe,
}

impl ProtocolHandler for Handshake {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        println!(
            "{{\"event\":\"handshake\",\"peer\":\"{}\",\"group\":\"{}\"}}",
            connection.remote_id(),
            self.probe.agreed().unwrap_or("none")
        );
        let Ok((mut send, mut recv)) = connection.accept_bi().await else {
            return Ok(());
        };
        let mut bytes = [0u8; 2];
        if recv.read_exact(&mut bytes).await.is_ok() {
            let _ = send.write_all(&bytes).await;
            let _ = send.finish();
        }
        // Keep the connection until the peer closes it, or the reply can be dropped with it.
        connection.closed().await;
        Ok(())
    }
}
