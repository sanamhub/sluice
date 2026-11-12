//! Delta transfers in the engine, as `SPEC.md` section 7 states them: a receiver that already holds
//! an older version of a file asks the sender for chunk signatures of the new one, rebuilds what it
//! can from its own file, and fetches the rest with the static protocol's verified ranges.
//!
//! The hint never decides anything. The rebuilt file is hashed and compared with the root the
//! manifest names before it enters the store, and any failure, refusal or oddity on the way ends in
//! the plain fetch, so a wrong or hostile hint costs time and nothing else.
//!
//! What is held, for a target of any size: one block of the file being read and at most one
//! maximum-size chunk (at most 64 MiB), the signature list (at most 20 MiB of bytes and 24 MiB
//! decoded), the candidate's chunk table (at most [`CANDIDATE_ENTRIES`] entries, about 64 MiB, and
//! its index, about the same again), and the need-ranges (at most [`MAX_NEED_RANGES`] ranges). None
//! of it grows with the target: a sender cuts a large target with a larger average chunk so that its
//! list fits, and a receiver reads only that many chunks of the candidate.

use std::time::Duration;

use sluice_delta::{AVG_BYTES, MAX_ENTRIES, Params};

pub(super) mod ask;
mod candidate;
mod list;
mod rebuild;
mod serve;

pub(super) use candidate::{Candidate, candidate_path};
#[cfg(test)]
pub(super) use rebuild::worth_it;
pub(super) use rebuild::{Job, Totals, fetch_file};
pub(crate) use serve::Delta;

/// Where the bytes of a receive that was given `--delta-from` came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeltaReport {
    /// Bytes copied from the local file.
    pub local_bytes: u64,
    /// Bytes fetched from the sender: the ranges a delta asked for, and every file that was fetched
    /// whole because no delta was used for it.
    pub network_bytes: u64,
    /// Bytes of signature lists the sender wrote.
    pub signature_bytes: u64,
}

/// How long the control exchange of one file may take besides the list's own build: from the
/// connection coming up to the end of the list.
pub(super) const CONTROL_BASE: Duration = Duration::from_secs(60);
/// The rate a sender is assumed to read and hash a blob at, which sets how long a requester waits for
/// the list of a large one.
pub(super) const LIST_RATE: u64 = 64 << 20;

/// How long a requester waits for the list of a blob of `size` bytes: [`CONTROL_BASE`] plus the size
/// at [`LIST_RATE`]. A peer that stalls costs this and then the plain fetch.
pub(super) fn control_wait(size: u64) -> Duration {
    CONTROL_BASE + Duration::from_secs(size / LIST_RATE)
}
/// How long a provider waits for the root after the greeting, and for the need-ranges after the
/// list.
pub(super) const REQUEST_WAIT: Duration = Duration::from_secs(10);
/// The most chunks of a candidate that are read. A larger candidate is cut only this far, which
/// leaves the rest of it unused and the plan with more to fetch.
pub(super) const CANDIDATE_ENTRIES: usize = 1 << 21;
/// The most need-ranges a receiver builds into one request. A plan with more is not worth its
/// request, and the file is fetched plainly.
pub(super) const MAX_NEED_RANGES: usize = 16_384;
/// The longest a signature list may be, as one frame: the algorithm, the average, a count of ten
/// bytes at most, and 20 bytes for each of [`MAX_ENTRIES`] entries.
pub(super) const MAX_SIG_FRAME: u64 = 1 + 4 + 10 + 20 * MAX_ENTRIES as u64;
/// The longest a need-ranges may be, as one frame: the root, a count, and 16 bytes for each range.
pub(super) const MAX_NEED_FRAME: u64 = 32 + 10 + 16 * MAX_ENTRIES as u64;
/// A file is not asked about when fewer than this share of it, in eighths, could be rebuilt from the
/// candidate. Below it a plain fetch moves about the same bytes and skips the assembly.
pub(super) const WORTH_EIGHTHS: u64 = 1;
/// The block a file is read and written in.
pub(super) const BLOCK: usize = 1 << 20;

/// The average chunk size a provider cuts `size` bytes with, for a requester whose frames may be at
/// most `frame_limit` bytes. It is the smallest power of two from [`AVG_BYTES`] up for which the
/// worst case, a file cut into chunks of the minimum length `avg / 4`, still fits both the entry
/// limit and the frame. `None` is a file too large for any average, which is not signed.
pub(super) fn params_for(size: u64, frame_limit: u32) -> Option<Params> {
    let by_frame = u64::from(frame_limit).saturating_sub(15) / 20;
    let most = by_frame.min(u64::from(MAX_ENTRIES));
    let mut avg = AVG_BYTES;
    loop {
        let params = Params::new(avg).ok()?;
        let worst = size.div_ceil(u64::from(params.min())).max(1);
        if worst <= most {
            return Some(params);
        }
        avg = avg.checked_mul(2)?;
    }
}

/// The root a delta request stream holds, as `SPEC.md` section 7 spells it: the 32 bytes of the
/// content root and nothing after them. `stream` is everything the requester wrote on the greeting's
/// stream before it ended its side. Any other stream is refused.
///
/// # Errors
///
/// `Error::Policy` when the stream is not exactly 32 bytes.
///
/// # Examples
///
/// ```
/// use sluice_core::engine::parse_delta_request;
///
/// assert_eq!(parse_delta_request(&[7; 32]).unwrap(), [7; 32]);
/// assert!(parse_delta_request(&[7; 33]).is_err());
/// assert!(parse_delta_request(&[7; 31]).is_err());
/// ```
pub fn parse_request(stream: &[u8]) -> Result<[u8; 32], crate::Error> {
    stream
        .try_into()
        .map_err(|_| crate::Error::Policy("a delta request is the 32 bytes of a root".to_string()))
}

/// Streams the bytes `range` of the blob `hash` out of the store, in the order they lie, handing each
/// piece and its offset to `each`. A `BlobReader` would do the same a piece at a time, but it starts
/// a new export for every read and a read is only a few kilobytes, which costs seconds for a
/// megabyte; one export for the whole range is the same bytes at the store's own speed.
pub(super) async fn export(
    store: &iroh_blobs::store::fs::FsStore,
    hash: iroh_blobs::Hash,
    range: std::ops::Range<u64>,
    mut each: impl FnMut(u64, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use iroh_blobs::api::proto::ExportRangesItem;
    use n0_future::StreamExt;

    let mut stream = std::pin::pin!(store.blobs().export_ranges(hash, range).stream());
    while let Some(item) = stream.next().await {
        match item {
            ExportRangesItem::Size(_) => {}
            ExportRangesItem::Data(leaf) => each(leaf.offset, &leaf.data)?,
            ExportRangesItem::Error(e) => {
                return Err(std::io::Error::other(format!("cannot read the blob: {e}")));
            }
        }
    }
    Ok(())
}

/// A frame as it travels, as a record of section 6 does: its length as a varint, then its bytes.
pub(super) fn frame(bytes: &[u8]) -> Vec<u8> {
    let mut out = super::hello::varint_bytes(bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

/// Reads one frame, refusing a length of zero or above `limit` before anything is reserved for it.
/// The bytes are taken in pieces, so a peer that declares a length it does not send holds the
/// memory it actually sent and no more.
pub(super) async fn read_frame(
    recv: &mut iroh::endpoint::RecvStream,
    limit: u64,
) -> std::io::Result<Vec<u8>> {
    const PIECE: usize = 64 << 10;
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_string());
    let len = super::hello::read_stream_varint(recv).await?;
    if len == 0 || len > limit {
        return Err(bad("a delta frame is empty or longer than the limit"));
    }
    let len = usize::try_from(len).map_err(|_| bad("a delta frame is too long"))?;
    let mut bytes = Vec::with_capacity(len.min(PIECE));
    let mut piece = vec![0u8; len.min(PIECE)];
    while bytes.len() < len {
        let want = (len - bytes.len()).min(PIECE);
        match recv.read(&mut piece[..want]).await {
            Ok(Some(got)) => bytes.extend_from_slice(&piece[..got]),
            Ok(None) => return Err(bad("a delta frame ends early")),
            Err(e) => return Err(std::io::Error::other(e)),
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests;
