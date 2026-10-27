//! One byte range of a one-file transfer. Only the blocks the range needs are fetched and verified
//! against the root, and only the requested bytes are written.

use std::path::{Path, PathBuf};

use iroh_blobs::{Hash, protocol::ChunkRangesExt, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::{
    Event, ReceiveOptions, Receiver, fetch,
    recv::{check_one, fetch_manifest, refuse_expired, relay_of, write_range},
    storage,
};
use crate::{
    Error,
    manifest::{Loc, Manifest},
    range::{ByteRange, FetchPlan},
    ticket::{Body, Format, Ticket},
};

/// Writes the bytes of `range` from a one-file transfer, and returns how many. A folder is refused:
/// a range names bytes of one file. The store is a private temporary one under `store_root`, and
/// its room is checked against what the range will fetch before the fetch starts.
pub async fn receive_range(
    text: &str,
    range: ByteRange,
    options: &ReceiveOptions,
    store_root: &Path,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    std::fs::create_dir_all(&options.out)
        .map_err(|e| storage::policy_path("create", &options.out, e))?;
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let tmp = match storage::temp_dir(store_root) {
        Ok(path) => Temp(path),
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let store = match FsStore::load(tmp.0.join("store")).await {
        Ok(store) => store,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::plain(&e)
            )));
        }
    };
    let result = async {
        let conn = receiver.connect(&ticket, events.clone()).await?;
        fetch_and_write(&conn, &ticket, &store, range, options, store_root, events).await
    }
    .await;
    let endpoint = receiver.shutdown().await;
    let closed = store.shutdown().await;
    // Removed before the result is returned, so no path out of this function leaves the folder.
    drop(tmp);
    let bytes = result?;
    if endpoint.is_err() || closed.is_err() {
        return Err(Error::Storage(
            "the bytes were written but the transfer did not close cleanly".to_string(),
        ));
    }
    Ok(bytes)
}

async fn fetch_and_write(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    range: ByteRange,
    options: &ReceiveOptions,
    store_root: &Path,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    if !matches!(
        ticket.body(),
        Body::Static {
            format: Format::HashSequence,
            ..
        }
    ) {
        return Err(Error::Policy(
            "a range needs a one-file transfer, and this ticket is not one".to_string(),
        ));
    }
    let manifest = fetch_manifest(conn, ticket, store, options.limits, events).await?;
    let entry = check_one(&manifest, options, false)?.clone();
    let resolved = range.resolve(entry.size)?;
    let plan = resolved.plan();
    let request = plan_fetch(&manifest, &entry, &plan)?;
    storage::ensure_space(store_root, request.need)?;
    fetch::fetch_range(store, conn, request.hash, request.ranges).await?;
    let bytes = plan.keep();
    write_range(
        store,
        request.hash,
        request.base + plan.skip(),
        bytes,
        &entry.path.under(&options.out),
        entry.mtime_ms,
        entry.mode,
        options,
        events,
    )
    .await?;
    Ok(bytes)
}

/// What one range request asks for: the blob or pack to read, the blocks the range touches, where
/// it starts in that blob, and how many bytes the store has to hold for it.
struct Fetch {
    hash: Hash,
    ranges: iroh_blobs::protocol::ChunkRanges,
    base: u64,
    need: u64,
}

fn plan_fetch(
    manifest: &Manifest,
    entry: &crate::manifest::Entry,
    plan: &FetchPlan,
) -> Result<Fetch, Error> {
    let fetched = plan.fetched();
    match entry.loc {
        Loc::Blob { root } => Ok(Fetch {
            hash: Hash::from(root),
            ranges: iroh_blobs::protocol::ChunkRanges::bytes(fetched.clone()),
            base: fetched.start,
            need: fetched.end.saturating_sub(fetched.start),
        }),
        Loc::Packed {
            pack_index, offset, ..
        } => {
            let pack = manifest.packs().get(pack_index as usize).ok_or_else(|| {
                Error::Policy("the manifest points at a pack that is not listed".to_string())
            })?;
            let start = offset.checked_add(fetched.start).ok_or_else(overflow)?;
            let end = offset.checked_add(fetched.end).ok_or_else(overflow)?;
            Ok(Fetch {
                hash: Hash::from(pack.root),
                ranges: iroh_blobs::protocol::ChunkRanges::bytes(start..end),
                base: start,
                need: pack.size,
            })
        }
        Loc::None => Err(Error::Bug(
            "a File entry has no location to write".to_string(),
        )),
    }
}

fn overflow() -> Error {
    Error::Policy("a byte range adds up past u64::MAX".to_string())
}

/// The temporary store, removed however the write ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        storage::remove_quietly(&self.0);
    }
}
