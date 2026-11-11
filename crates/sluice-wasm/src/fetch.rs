//! The download itself: the relay's answer, then the bytes the ticket names, bounded by progress.

use iroh::{Endpoint, EndpointId};
use iroh_blobs::{
    Hash, api::downloader::Shuffled, api::proto::BlobStatus, protocol::ChunkRanges,
    protocol::ChunkRangesExt, protocol::GetRequest, store::mem::MemStore,
};
use n0_future::time::timeout;

use crate::{Bad, IDLE_WAIT, RELAY_WAIT};

/// One fetch through an endpoint that is already bound: the relay's answer first, and then the
/// download.
pub(super) async fn fetch(
    endpoint: &Endpoint,
    sender: EndpointId,
    hash: Hash,
    size: u64,
) -> Result<Vec<u8>, Bad> {
    // The relay connection has to be up before a peer can be found through it: a browser has no
    // direct path, so the relay is the only way to the sender.
    timeout(RELAY_WAIT, endpoint.online()).await.map_err(|_| {
        Bad(format!(
            "the relay did not answer in {} s",
            RELAY_WAIT.as_secs()
        ))
    })?;
    fetch_direct(endpoint, sender, hash, size).await
}

/// The download itself, through an endpoint that can already reach the sender. The store is this
/// call's and is dropped with it, so the bytes exist in the page only as the value returned here,
/// and only the bytes the ticket names are ever asked for.
pub(super) async fn fetch_direct(
    endpoint: &Endpoint,
    sender: EndpointId,
    hash: Hash,
    size: u64,
) -> Result<Vec<u8>, Bad> {
    let store = MemStore::new();
    // The downloader takes the peers that may have the content; the endpoint id is enough, because
    // a relayed connection is the only kind a browser makes and the relay knows where it lives.
    // The request names the byte range the ticket promises, so a peer cannot make the tab hold more
    // than that: content of another length leaves the store incomplete and is refused below.
    // Every attempt has [`IDLE_WAIT`] to make the store grow, and an attempt that did not grow it is
    // the last: a peer that stops sending fails the promise, while one that sends slowly keeps the
    // fetch going for as long as it delivers. The downloader resumes from the store, so an attempt
    // that timed out and one that was never made ask for the bytes that are still missing.
    let mut reached = 0u64;
    loop {
        let request = GetRequest::blob_ranges(hash, ChunkRanges::bytes(..size));
        let peers = Shuffled::new(vec![sender]);
        match timeout(
            IDLE_WAIT,
            store.downloader(endpoint).download(request, peers),
        )
        .await
        {
            Ok(result) => {
                result.map_err(|e| Bad(format!("the fetch stopped: {e}")))?;
                break;
            }
            Err(_) => {
                let stored = match store.blobs().status(hash).await {
                    Ok(BlobStatus::Partial { size: stored }) => stored.unwrap_or(0),
                    Ok(BlobStatus::Complete { size: stored }) => stored,
                    _ => 0,
                };
                if stored <= reached {
                    return Err(Bad(format!(
                        "the fetch made no progress in {} s",
                        IDLE_WAIT.as_secs()
                    )));
                }
                reached = stored;
            }
        }
    }
    let bytes = match store.blobs().status(hash).await {
        Ok(BlobStatus::Complete { size: stored }) => {
            if stored != size {
                return Err(Bad(format!(
                    "the content is {stored} bytes, and the ticket names {size}"
                )));
            }
            store
                .blobs()
                .get_bytes(hash)
                .await
                .map_err(|e| Bad(format!("the blob cannot be read: {e}")))?
        }
        // The request asked for the bytes the ticket names and no others, so a blob that is only
        // partly there is content that is longer than the ticket says.
        Ok(BlobStatus::Partial { .. }) => {
            return Err(Bad(format!(
                "the content is longer than the {size} bytes the ticket names"
            )));
        }
        Ok(BlobStatus::NotFound) => {
            return Err(Bad("the fetch left no content in the store".to_string()));
        }
        Err(error) => {
            return Err(Bad(format!("the stored blob cannot be read: {error}")));
        }
    };
    Ok(bytes.to_vec())
}
