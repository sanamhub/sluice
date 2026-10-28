//! Every record of a live stream until its close: each frame adds its segment, and the close proves
//! the whole file.

use std::path::Path;

use iroh_blobs::{Hash, store::fs::FsStore};
use sluice_live::{Outcome, Receiver as LogReceiver};
use tokio::{io::AsyncWriteExt, sync::mpsc::UnboundedSender};

use super::super::read_record;
use super::limits::{Bounds, Stop};
use crate::{
    Error,
    engine::{Event, fetch, storage},
};

/// Every record until the close: a frame adds its segment, and the close proves the whole file.
#[allow(clippy::too_many_arguments)]
pub(super) async fn records(
    log: &mut LogReceiver,
    recv: &mut iroh::endpoint::RecvStream,
    store: &FsStore,
    blobs: &iroh::endpoint::Connection,
    file: &mut tokio::fs::File,
    hasher: &mut blake3::Hasher,
    bounds: &Bounds,
    rate: Option<&crate::engine::Rate>,
    events: &UnboundedSender<Event>,
) -> Result<u64, Stop> {
    let mut total = 0u64;
    loop {
        let record = read_record(recv).await?;
        match record.len() {
            sluice_live::Frame::LEN => {
                total = apply_frame(
                    log, &record, store, blobs, file, hasher, bounds, events, rate, total,
                )
                .await?;
            }
            sluice_live::LogClose::LEN => {
                return close(log, &record, total, hasher).map_err(Stop::from);
            }
            _ => {
                return Err(Stop::from(Error::Verify(
                    "a live record is not one of the three the stream carries".to_string(),
                )));
            }
        }
    }
}

/// One frame: check it against the limits, fetch its segment, and add it to the file. A repeated
/// entry changes nothing; one that breaks the chain or comes out of order stops the transfer.
#[allow(clippy::too_many_arguments)]
async fn apply_frame(
    log: &mut LogReceiver,
    record: &[u8],
    store: &FsStore,
    blobs: &iroh::endpoint::Connection,
    file: &mut tokio::fs::File,
    hasher: &mut blake3::Hasher,
    bounds: &Bounds,
    events: &UnboundedSender<Event>,
    rate: Option<&crate::engine::Rate>,
    total: u64,
) -> Result<u64, Stop> {
    let outcome = log.frame(record);
    let Outcome::Accepted {
        segment_root, len, ..
    } = outcome
    else {
        return match outcome {
            Outcome::Repeat { .. } => Ok(total),
            Outcome::Refused(fault) => {
                Err(Error::Verify(format!("the live chain refused a frame: {fault}")).into())
            }
            Outcome::Aborted { seq, reason } => {
                Err(Error::Verify(format!("the live chain was aborted at {seq}: {reason}")).into())
            }
            _ => Err(Error::Verify("a live frame did not change the stream".to_string()).into()),
        };
    };
    // The limits are checked before the segment is fetched, so a stream that would pass one stops
    // without spending the room or the bandwidth on bytes that cannot be written.
    bounds.check(total, len)?;
    let hash = Hash::from(segment_root);
    // Held only until this segment is written out. The collector frees it after that, so a long
    // stream keeps one segment beside the file it is writing and not every segment it has seen.
    let held = store
        .tags()
        .temp_tag(iroh_blobs::HashAndFormat::raw(hash))
        .await
        .map_err(|e| Error::Storage(format!("cannot hold a live segment: {}", crate::plain(&e))))?;
    fetch::content_blob(store, blobs, hash, u64::from(len), 0, 0, rate, events).await?;
    let bytes = fetch::complete_bytes(store, hash).await?.0;
    file.write_all(&bytes)
        .await
        .map_err(|e| storage::storage("write", Path::new("the live file"), e))?;
    drop(held);
    hasher.update(&bytes);
    total
        .checked_add(u64::from(len))
        .ok_or_else(|| Error::Verify("the live file is longer than u64::MAX".to_string()).into())
}

/// The close: the length and the root have to match everything that arrived.
fn close(
    log: &mut LogReceiver,
    record: &[u8],
    total: u64,
    hasher: &mut blake3::Hasher,
) -> Result<u64, Error> {
    match log.close(record) {
        Outcome::Closed {
            final_root,
            total_len,
        } => {
            if total_len != total {
                return Err(Error::Verify(format!(
                    "the close says {total_len} bytes and the segments add up to {total}"
                )));
            }
            if *hasher.finalize().as_bytes() != final_root {
                return Err(Error::Verify(
                    "the file does not hash to the root the close names".to_string(),
                ));
            }
            Ok(total)
        }
        Outcome::Aborted { seq, reason } => Err(Error::Verify(format!(
            "the live chain was aborted at {seq}: {reason}"
        ))),
        other => Err(Error::Verify(format!(
            "the live close was refused: {other:?}"
        ))),
    }
}
