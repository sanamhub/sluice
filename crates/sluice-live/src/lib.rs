//! The segment log for files that are still being written, as `SPEC.md` section 6 states it.
//!
//! A sender seals a growing file into segments and signs one entry per segment, each naming the
//! one before, so the entries form a chain that starts at a signed [`LogOpen`] and ends at a
//! signed [`LogClose`]. [`Sender`] makes those records and [`Receiver`] checks them. A receiver
//! gets the records from providers it does not trust: a record that a provider could have made
//! is refused and changes nothing, and only a record the sender signed can abort the stream.
//!
//! The crate is sans-IO. It touches no network, filesystem or clock: bytes go in and outcomes
//! come out, so the browser build uses it unchanged. Fetching segments, and the provisional
//! tail, belong to the transfer layer, which this crate hands the sealed `segment_root` and `len`.

mod error;
mod receiver;
mod records;
mod sender;
mod sig;

pub use error::{Error, Fault};
pub use receiver::{Outcome, Receiver, Status};
pub use records::{Frame, LogClose, LogEntry, LogOpen};
pub use sender::Sender;

/// The size of a full segment that a sender uses unless it is told another: 8 MiB.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_live::SEGMENT_BYTES, 8 << 20);
/// ```
pub const SEGMENT_BYTES: u32 = 8 << 20;

/// The smallest `segment_bytes`, 1 MiB. A receiver keeps 32 bytes per entry, so this bounds its
/// memory at 32 MiB for a file of 1 TB.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_live::MIN_SEGMENT_BYTES, 1 << 20);
/// ```
pub const MIN_SEGMENT_BYTES: u32 = 1 << 20;

/// The largest `segment_bytes`, 64 MiB.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_live::MAX_SEGMENT_BYTES, 64 << 20);
/// ```
pub const MAX_SEGMENT_BYTES: u32 = 64 << 20;

/// `segment_bytes` is a multiple of this, the block size of `iroh-blobs`, so a range inside a
/// segment is a range of whole blocks.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_live::SEGMENT_BYTES % sluice_live::SEGMENT_BLOCK, 0);
/// ```
pub const SEGMENT_BLOCK: u32 = 16 << 10;

fn segment_bytes_in_bounds(segment_bytes: u32) -> bool {
    (MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&segment_bytes)
        && segment_bytes.is_multiple_of(SEGMENT_BLOCK)
}
