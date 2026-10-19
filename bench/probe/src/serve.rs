//! `probe serve`: serve generated data and print one ticket line.

use anyhow::{Context, Result};
use iroh::protocol::Router;
use iroh_blobs::{BlobsProtocol, store::mem::MemStore};

use crate::{
    cli::{Mode, ServeArgs},
    data, net,
    raw::RawHandler,
    ticket::Ticket,
};

/// Serves until the process is killed.
pub async fn run(args: ServeArgs) -> Result<()> {
    let endpoint = net::bind(&args.shared).await?;
    let builder = Router::builder(endpoint.clone());

    let (router, hash) = match args.shared.mode {
        Mode::Raw => {
            let handler = RawHandler::new(args.seed, args.size)?;
            (builder.accept(net::RAW_ALPN, handler).spawn(), None)
        }
        Mode::Blobs => {
            // In memory so the disk is not part of what the receiver measures.
            let store = MemStore::new();
            let tag = store
                .add_bytes(data::generate(args.seed, args.size)?)
                .await
                .context("import test data")?;
            let blobs = BlobsProtocol::new(&store, None);
            (
                builder.accept(iroh_blobs::ALPN, blobs).spawn(),
                Some(tag.hash.to_hex()),
            )
        }
    };

    let ticket = Ticket {
        mode: args.shared.mode,
        addr: net::dialable_addr(&endpoint, &args.shared.relay).await?,
        size: args.size,
        seed: args.seed,
        hash,
    };
    println!("{}", ticket.encode()?);
    tracing::info!(mode = ?args.shared.mode, size = args.size, "serving");

    tokio::signal::ctrl_c().await.context("wait for ctrl-c")?;
    router.shutdown().await.context("shut down")?;
    Ok(())
}
