//! `probe serve`: serve generated data and print one ticket line.

use anyhow::{Context, Result, anyhow, ensure};
use iroh::protocol::Router;
use iroh_blobs::{BlobsProtocol, store::mem::MemStore};

use crate::{
    cli::{Mode, ServeArgs, StoreKind},
    data, disk, net,
    raw::RawHandler,
    ticket::Ticket,
};

/// Serves until the process is killed.
pub async fn run(args: ServeArgs) -> Result<()> {
    ensure!(
        args.store == StoreKind::Mem || args.shared.mode == Mode::Blobs,
        "--store fs needs --mode blobs, raw mode has no store"
    );
    // Every endpoint serves the same data, so which address a fetch dials changes the socket
    // it uses and nothing else.
    let mut endpoints = Vec::new();
    for _ in 0..args.endpoints {
        endpoints.push(net::bind(&args.shared).await?);
    }

    let mut routers: Vec<Router> = Vec::new();
    let mut hash = None;
    // Kept so the stores outlive the routers.
    let mut mem_store = None;
    let mut fs_store = None;
    match args.shared.mode {
        Mode::Raw => {
            let handler = RawHandler::new(args.seed, args.size)?;
            for endpoint in &endpoints {
                let router = Router::builder(endpoint.clone());
                routers.push(router.accept(net::RAW_ALPN, handler.clone()).spawn());
            }
        }
        Mode::Blobs => {
            let blobs = match args.store {
                StoreKind::Mem => {
                    // In memory so the disk is not part of what the receiver measures.
                    let store = MemStore::new();
                    let tag = store
                        .add_bytes(data::generate(args.seed, args.size)?)
                        .await
                        .context("import test data")?;
                    hash = Some(tag.hash);
                    let blobs = BlobsProtocol::new(&store, None);
                    mem_store = Some(store);
                    blobs
                }
                StoreKind::Fs => {
                    let dir = args.dir.as_deref().context("--store fs needs --dir")?;
                    let provider = disk::open(dir, args.seed, args.size).await?;
                    hash = Some(provider.hash);
                    let blobs = BlobsProtocol::new(&provider.store, None);
                    fs_store = Some(provider.store);
                    blobs
                }
            };
            for endpoint in &endpoints {
                let router = Router::builder(endpoint.clone());
                routers.push(router.accept(iroh_blobs::ALPN, blobs.clone()).spawn());
            }
        }
    }

    let mut addrs = Vec::new();
    for endpoint in &endpoints {
        addrs.push(net::dialable_addr(endpoint, &args.shared.relay).await?);
    }
    let ticket = Ticket {
        mode: args.shared.mode,
        addrs,
        size: args.size,
        seed: args.seed,
        hash: hash.map(|h| h.to_hex()),
        store: (args.shared.mode == Mode::Blobs).then_some(args.store),
    };
    println!("{}", ticket.encode()?);
    tracing::info!(
        mode = ?args.shared.mode,
        store = ?args.store,
        size = args.size,
        endpoints = args.endpoints,
        "serving"
    );

    tokio::signal::ctrl_c().await.context("wait for ctrl-c")?;
    for router in routers {
        router.shutdown().await.context("shut down")?;
    }
    if let Some(store) = fs_store {
        store
            .shutdown()
            .await
            .map_err(|e| anyhow!("close store: {e}"))?;
    }
    drop(mem_store);
    Ok(())
}
