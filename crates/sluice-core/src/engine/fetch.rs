//! Fetching bytes from a provider. The size of a blob is proved by its first chunk group, so a
//! receiver can refuse an oversized manifest before it fills its store with it.

use std::pin::pin;

use iroh_blobs::{
    Hash, HashAndFormat,
    api::{proto::BlobStatus, remote::GetProgressItem},
    get::{GetError, fsm::DecodeError},
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
    fetch_blob(store, conn, hash, 0, 0, events, None).await?;
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
    size_of(store, hash)
        .await?
        .ok_or_else(|| Error::Verify("the provider did not prove the size of the blob".to_string()))
}

/// Fetches a content blob after proving it has the size the manifest gives it. A blob that is
/// larger or smaller than declared is refused before its body is requested, so a hostile provider
/// cannot fill the store with bytes the manifest never accounted for.
#[allow(clippy::too_many_arguments)]
pub(super) async fn content_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    expected: u64,
    base: u64,
    total: u64,
    rate: Option<&super::Rate>,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let size = verified_size(store, conn, hash).await?;
    if size != expected {
        return Err(Error::Verify(format!(
            "a blob has {size} bytes and the manifest says {expected}"
        )));
    }
    fetch_blob(store, conn, hash, base, total, events, rate).await
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
        .map_err(classify)
}

/// Fetches a whole blob with an explicit request, even when the store already holds it, so the
/// provider sees a request that covers the whole blob and a raw receive has something to count.
pub(super) async fn fetch_whole(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
) -> Result<(), Error> {
    store
        .remote()
        .execute_get(conn.clone(), GetRequest::blob(hash))
        .complete()
        .await
        .map(|_| ())
        .map_err(classify)
}
/// Fetches a range of a blob, verified block by block against its root, and waits under `rate`
/// for the bytes of each step. The range is in bytes and is rounded up to whole blocks, which is
/// the smallest unit `iroh-blobs` verifies.
pub(super) async fn fetch_range(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    ranges: ChunkRanges,
    rate: Option<&super::Rate>,
) -> Result<(), Error> {
    let mut progress = pin!(
        store
            .remote()
            .execute_get(conn.clone(), GetRequest::blob_ranges(hash, ranges))
            .stream()
    );
    let mut seen = 0u64;
    while let Some(item) = progress.next().await {
        match item {
            GetProgressItem::Progress(bytes) => {
                if let Some(rate) = rate {
                    rate.acquire(bytes.saturating_sub(seen)).await;
                }
                seen = bytes;
            }
            GetProgressItem::Done(_) => return Ok(()),
            GetProgressItem::Error(e) => return Err(classify(e)),
        }
    }
    Err(Error::Network(
        "the connection closed before the range completed".to_string(),
    ))
}
/// The size a store has proved for a blob, if it has.
async fn size_of(store: &FsStore, hash: Hash) -> Result<Option<u64>, Error> {
    Ok(
        match store.blobs().status(hash).await.map_err(|e| {
            Error::Storage(format!(
                "cannot read the partial store: {}",
                crate::plain(&e)
            ))
        })? {
            BlobStatus::Complete { size } => Some(size),
            BlobStatus::Partial { size } => size,
            BlobStatus::NotFound => None,
        },
    )
}

/// Fetches a whole blob, reporting `base` plus the bytes seen against `total` when it is known,
/// and waiting under `rate` for the bytes of each step before it asks for more.
pub(super) async fn fetch_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    base: u64,
    total: u64,
    events: &UnboundedSender<Event>,
    rate: Option<&super::Rate>,
) -> Result<(), Error> {
    let mut progress = pin!(
        store
            .remote()
            .fetch(conn.clone(), HashAndFormat::raw(hash))
            .stream()
    );
    let mut seen = 0u64;
    while let Some(item) = progress.next().await {
        match item {
            GetProgressItem::Progress(bytes) => {
                if let Some(rate) = rate {
                    rate.acquire(bytes.saturating_sub(seen)).await;
                }
                seen = bytes;
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
            GetProgressItem::Error(e) => return Err(classify(e)),
        }
    }
    Err(Error::Network(
        "the connection closed before the blob completed".to_string(),
    ))
}

/// A get error is a verification failure only when the provider sent bytes that did not match the
/// root; everything else is a connection that can be retried.
fn classify(e: GetError) -> Error {
    match &e {
        GetError::Decode {
            source: DecodeError::LeafHashMismatch { .. } | DecodeError::ParentHashMismatch { .. },
            ..
        } => Error::Verify("received bytes did not match the ticket root".to_string()),
        _ => Error::Network(format!("the transfer stopped: {e}")),
    }
}

/// The size of a blob the store has, without reading its bytes into memory.
pub(super) async fn complete_size(store: &FsStore, hash: Hash) -> Result<u64, Error> {
    match store.blobs().status(hash).await.map_err(|e| {
        Error::Storage(format!(
            "cannot read the partial store: {}",
            crate::plain(&e)
        ))
    })? {
        BlobStatus::Complete { size } => Ok(size),
        _ => Err(Error::Verify(
            "the transfer ended without the whole blob".to_string(),
        )),
    }
}

/// The verified bytes and size of a blob already in the store.
pub(super) async fn complete_bytes(store: &FsStore, hash: Hash) -> Result<(Vec<u8>, u64), Error> {
    let status = store.blobs().status(hash).await.map_err(|e| {
        Error::Storage(format!(
            "cannot read the partial store: {}",
            crate::plain(&e)
        ))
    })?;
    match status {
        BlobStatus::Complete { size } => {
            let bytes = store.blobs().get_bytes(hash).await.map_err(|e| {
                Error::Storage(format!(
                    "cannot read the received blob: {}",
                    crate::plain(&e)
                ))
            })?;
            Ok((bytes.to_vec(), size))
        }
        _ => Err(Error::Verify(
            "the transfer ended without the whole blob".to_string(),
        )),
    }
}
