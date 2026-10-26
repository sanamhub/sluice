//! Fetching bytes from a provider. The size of a blob is proved by its first chunk group, so a
//! receiver can refuse an oversized manifest before it fills its store with it.

use std::pin::pin;

use iroh_blobs::{
    Hash, HashAndFormat,
    api::{proto::BlobStatus, remote::GetProgressItem},
    protocol::{ChunkRanges, ChunkRangesExt, GetRequest},
    store::fs::FsStore,
};
use n0_future::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use super::Event;
use crate::{Error, manifest::Limits};

/// Fetches `hash` and returns its verified bytes after checking the size against `limits`. The
/// first request asks for one chunk group, whose BAO proof carries the total size; only when that
/// size fits is the rest requested.
pub(super) async fn bounded_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    limits: Limits,
    events: &UnboundedSender<Event>,
) -> Result<Vec<u8>, Error> {
    let size = verified_size(store, conn, hash).await?;
    limits.check_bytes(size)?;
    fetch_blob(store, conn, hash, 0, 0, events).await?;
    let (bytes, got) = complete_bytes(store, hash).await?;
    if got != size {
        return Err(Error::Verify(format!(
            "the blob has {got} bytes and its first chunk proved {size}"
        )));
    }
    Ok(bytes)
}

/// The total size of `hash`, proved by the first chunk group. A provider that will not prove it
/// is refused rather than allowed to fill the store.
pub(super) async fn verified_size(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
) -> Result<u64, Error> {
    // The size is in the first chunk group, but this store records it only once the last chunk
    // arrives, because that is what proves the tree shape. Ask for the first chunk; a blob small
    // enough to be that one chunk is complete, and only a larger one needs the last chunk too.
    request(store, conn, hash, ChunkRanges::chunk(0)).await?;
    if let Some(size) = size_of(store, hash).await? {
        return Ok(size);
    }
    request(store, conn, hash, ChunkRanges::last_chunk()).await?;
    size_of(store, hash).await?.ok_or_else(|| {
        Error::Verify("the provider did not prove the size of the manifest".to_string())
    })
}

async fn request(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    ranges: ChunkRanges,
) -> Result<(), Error> {
    store
        .remote()
        .execute_get(conn.clone(), GetRequest::blob_ranges(hash, ranges))
        .complete()
        .await
        .map(|_| ())
        .map_err(|e| Error::Network(format!("the transfer stopped: {e}")))
}

/// The size a store has proved for a blob, if it has.
async fn size_of(store: &FsStore, hash: Hash) -> Result<Option<u64>, Error> {
    Ok(
        match store
            .blobs()
            .status(hash)
            .await
            .map_err(|e| Error::Storage(format!("cannot read the partial store: {e}")))?
        {
            BlobStatus::Complete { size } => Some(size),
            BlobStatus::Partial { size } => size,
            BlobStatus::NotFound => None,
        },
    )
}

/// Fetches a whole blob, reporting `base` plus the bytes seen against `total` when it is known.
pub(super) async fn fetch_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    base: u64,
    total: u64,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let mut progress = pin!(
        store
            .remote()
            .fetch(conn.clone(), HashAndFormat::raw(hash))
            .stream()
    );
    while let Some(item) = progress.next().await {
        match item {
            GetProgressItem::Progress(bytes) => {
                if total > 0 {
                    super::emit(
                        events,
                        Event::Progress {
                            bytes: base.saturating_add(bytes),
                            total,
                        },
                    );
                }
            }
            GetProgressItem::Done(_) => return Ok(()),
            GetProgressItem::Error(e) => {
                let text = e.to_string();
                return Err(if text.contains("hash") && text.contains("mismatch") {
                    Error::Verify("received bytes did not match the ticket root".to_string())
                } else {
                    Error::Network(format!("the transfer stopped: {text}"))
                });
            }
        }
    }
    Err(Error::Network(
        "the connection closed before the blob completed".to_string(),
    ))
}

/// The verified bytes and size of a blob already in the store.
pub(super) async fn complete_bytes(store: &FsStore, hash: Hash) -> Result<(Vec<u8>, u64), Error> {
    let status = store
        .blobs()
        .status(hash)
        .await
        .map_err(|e| Error::Storage(format!("cannot read the partial store: {e}")))?;
    match status {
        BlobStatus::Complete { size } => {
            let bytes = store
                .blobs()
                .get_bytes(hash)
                .await
                .map_err(|e| Error::Storage(format!("cannot read the received blob: {e}")))?;
            Ok((bytes.to_vec(), size))
        }
        _ => Err(Error::Verify(
            "the transfer ended without the whole blob".to_string(),
        )),
    }
}
