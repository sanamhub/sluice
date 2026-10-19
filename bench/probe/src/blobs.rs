//! Blobs mode: an `iroh-blobs` get split across connections, every byte verified.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use anyhow::{Context, Result, bail};
use iroh::endpoint::Connection;
use iroh_blobs::{
    Hash,
    api::{Store, remote::GetProgressItem},
    get::fsm::{self, BlobContentNext, ConnectedNext, EndBlobNext},
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
};
use n0_future::StreamExt;

/// bao chunks are 1 KiB.
const CHUNK_BYTES: u64 = 1024;
/// Verified ranges are quantized to 16 chunk groups (`docs/design.md`, Static content).
const GROUP_CHUNKS: u64 = 16;

/// Contiguous chunk ranges, one per connection, with every boundary on a 16 KiB group. A boundary
/// inside a group would make two connections each send it.
pub fn split(size: u64, parts: u16) -> Result<Vec<(u64, u64)>> {
    let chunks = size.div_ceil(CHUNK_BYTES);
    let groups = chunks.div_ceil(GROUP_CHUNKS);
    let parts = u64::from(parts);
    if groups < parts {
        bail!("{size} bytes cannot be split over {parts} connections in whole 16 KiB groups");
    }
    Ok((0..parts)
        .map(|i| {
            let start = groups * i / parts * GROUP_CHUNKS;
            let end = (groups * (i + 1) / parts * GROUP_CHUNKS).min(chunks);
            (start, end)
        })
        .collect())
}

fn request(hash: Hash, (start, end): (u64, u64)) -> GetRequest {
    GetRequest::blob_ranges(hash, ChunkRanges::chunks(start..end))
}

/// Fetches one range, verifies it, and drops it. Returns the payload bytes read, and keeps
/// `counter` current as each verified group arrives.
pub async fn fetch_discard(
    conn: Connection,
    hash: Hash,
    range: (u64, u64),
    counter: Arc<AtomicU64>,
) -> Result<u64> {
    let connected = fsm::start(conn, request(hash, range), Default::default())
        .next()
        .await
        .context("send request")?;
    let ConnectedNext::StartRoot(root) = connected.next().await.context("read response")? else {
        bail!("the provider answered a one-blob request with something else");
    };
    let (mut content, _size) = root.next().next().await.context("read blob header")?;
    let end = loop {
        match content.next().await {
            BlobContentNext::More((next, item)) => {
                item.context("verify content")?;
                counter.store(next.stats().payload_bytes_read, Ordering::Relaxed);
                content = next;
            }
            BlobContentNext::Done(end) => break end,
        }
    };
    let EndBlobNext::Closing(closing) = end.next() else {
        bail!("the provider sent more than one blob");
    };
    let stats = closing.next().await.context("finish request")?;
    Ok(stats.payload_bytes_read)
}

/// Fetches one range into `store`. Returns the payload bytes read, and keeps `counter` current
/// from the progress items the store reports.
pub async fn fetch_store(
    store: Store,
    conn: Connection,
    hash: Hash,
    range: (u64, u64),
    counter: Arc<AtomicU64>,
) -> Result<u64> {
    let mut items = std::pin::pin!(
        store
            .remote()
            .execute_get(conn, request(hash, range))
            .stream()
    );
    while let Some(item) = items.next().await {
        match item {
            GetProgressItem::Progress(bytes) => counter.store(bytes, Ordering::Relaxed),
            GetProgressItem::Done(stats) => {
                counter.store(stats.payload_bytes_read, Ordering::Relaxed);
                return Ok(stats.payload_bytes_read);
            }
            GetProgressItem::Error(e) => return Err(e).context("get into the store"),
        }
    }
    bail!("the get ended without a result")
}

#[cfg(test)]
mod tests {
    use super::split;

    #[test]
    fn parts_are_contiguous_group_aligned_and_cover_everything() {
        for (size, parts) in [
            (8 << 20, 1),
            (8 << 20, 4),
            ((8 << 20) + 1, 3),
            (1 << 30, 16),
        ] {
            let chunks = (size as u64).div_ceil(1024);
            let bounds = split(size as u64, parts).unwrap();
            assert_eq!(bounds.first().unwrap().0, 0);
            assert_eq!(bounds.last().unwrap().1, chunks);
            for pair in bounds.windows(2) {
                assert_eq!(pair[0].1, pair[1].0);
                assert_eq!(pair[1].0 % 16, 0);
            }
        }
    }

    #[test]
    fn too_small_for_the_connection_count_is_an_error() {
        assert!(split(32 << 10, 3).is_err());
    }
}
