//! One byte range of a one-file transfer. Only the blocks the range needs are fetched and verified
//! against the root, and only the requested bytes are written.

use std::path::PathBuf;

use iroh_blobs::{Hash, protocol::ChunkRangesExt, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::{
    Event, ReceiveOptions, Receiver, fetch,
    recv::{check_one, fetch_manifest, refuse_expired, relay_of, write_range},
    storage,
};
use crate::{
    Error,
    manifest::Loc,
    range::ByteRange,
    ticket::{Body, Format, Ticket},
};

/// Writes the bytes of `range` from a one-file transfer, and returns how many. A folder is refused:
/// a range names bytes of one file.
pub async fn receive_range(
    text: &str,
    range: ByteRange,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    std::fs::create_dir_all(&options.out)
        .map_err(|e| storage::policy_path("create", &options.out, e))?;
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let tmp = Temp(storage::temp_dir(&std::env::temp_dir())?);
    let store = match FsStore::load(tmp.0.join("store")).await {
        Ok(store) => store,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::chain(&e)
            )));
        }
    };
    let conn = receiver.connect(&ticket, events.clone()).await?;
    let result = fetch_and_write(&conn, &ticket, &store, range, options, events).await;
    let _ = receiver.shutdown().await;
    let _ = store.shutdown().await;
    result
}

async fn fetch_and_write(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    range: ByteRange,
    options: &ReceiveOptions,
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
    let fetched = plan.fetched();
    let target = options.out.join(entry.path.as_str());
    let (hash, base) = match entry.loc {
        Loc::Blob { root } => {
            let hash = Hash::from(root);
            fetch::fetch_range(
                store,
                conn,
                hash,
                iroh_blobs::protocol::ChunkRanges::bytes(fetched.clone()),
            )
            .await?;
            (hash, fetched.start)
        }
        Loc::Packed {
            pack_index, offset, ..
        } => {
            let pack = manifest.packs().get(pack_index as usize).ok_or_else(|| {
                Error::Policy("the manifest points at a pack that is not listed".to_string())
            })?;
            let start = offset.saturating_add(fetched.start);
            let end = offset.saturating_add(fetched.end);
            let hash = Hash::from(pack.root);
            fetch::fetch_range(
                store,
                conn,
                hash,
                iroh_blobs::protocol::ChunkRanges::bytes(start..end),
            )
            .await?;
            (hash, start)
        }
        Loc::None => {
            return Err(Error::Bug(
                "a File entry has no location to write".to_string(),
            ));
        }
    };
    let bytes = plan.keep();
    write_range(
        store,
        hash,
        base + plan.skip(),
        bytes,
        &target,
        entry.mtime_ms,
        entry.mode,
        options,
        events,
    )
    .await?;
    Ok(bytes)
}

/// The temporary store, removed however the write ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        storage::remove_quietly(&self.0);
    }
}
