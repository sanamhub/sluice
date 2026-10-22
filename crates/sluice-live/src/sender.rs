//! The sender's side: it signs the opening record, one frame per sealed segment, and the close.

use std::fmt;

use ed25519_dalek::SigningKey;

use crate::sig::{self, Label};
use crate::{Error, Fault, Frame, LogClose, LogEntry, LogOpen, segment_bytes_in_bounds};

/// Signs the records of one stream, in order, and refuses a call that would break a sender rule
/// of `SPEC.md` section 6.
///
/// It holds the key and the head of the chain. [`Sender::close`] takes it by value, so no entry
/// can be signed after the close.
///
/// # Examples
///
/// ```
/// use ed25519_dalek::SigningKey;
/// use sluice_live::{Error, Fault, Sender, SEGMENT_BYTES};
///
/// // The secret key of RFC 8032 section 7.1, TEST 1, which is published.
/// let key = SigningKey::from_bytes(&[
///     0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
///     0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
///     0x7f, 0x60,
/// ]);
/// let mut sender = Sender::new(key, [7; 16], SEGMENT_BYTES, 1_800_000_000_000)?;
/// let open = sender.log_open();
/// assert_eq!(open.segment_bytes, SEGMENT_BYTES);
///
/// let first = sender.seal([1; 32], SEGMENT_BYTES)?;
/// let last = sender.seal([2; 32], 10)?;
/// assert_eq!(last.entry.prev_entry_hash, first.entry.hash());
/// // A short segment was the last, so another one would break the chain.
/// assert_eq!(sender.seal([3; 32], 10), Err(Error::Policy(Fault::AfterShort)));
///
/// let close = sender.close([4; 32], 1_800_000_000_001);
/// assert_eq!(close.total_len, u64::from(SEGMENT_BYTES) + 10);
/// assert_eq!(close.last_entry_hash, last.entry.hash());
/// # Ok::<(), sluice_live::Error>(())
/// ```
pub struct Sender {
    key: SigningKey,
    open: LogOpen,
    head: [u8; 32],
    next: u64,
    total: u64,
    last_short: bool,
}

impl Sender {
    /// A sender for the stream `stream_id`, with segments of `segment_bytes`. It signs the
    /// `LogOpen` at once, and [`Sender::log_open`] gives it.
    ///
    /// # Errors
    ///
    /// [`Error::Policy`] with [`Fault::SegmentBytes`] when `segment_bytes` is not a multiple of
    /// 16,384 from 1 MiB to 64 MiB.
    ///
    /// # Examples
    ///
    /// See [`Sender`].
    pub fn new(
        key: SigningKey,
        stream_id: [u8; 16],
        segment_bytes: u32,
        started: u64,
    ) -> Result<Self, Error> {
        if !segment_bytes_in_bounds(segment_bytes) {
            return Err(Error::Policy(Fault::SegmentBytes));
        }
        let mut open = LogOpen {
            stream_id,
            sender_key: key.verifying_key().to_bytes(),
            segment_bytes,
            started,
            sig: [0; 64],
        };
        open.sig = sig::sign(&key, Label::Open, &stream_id, &open.signed());
        Ok(Self {
            key,
            head: open.hash(),
            open,
            next: 0,
            total: 0,
            last_short: false,
        })
    }

    /// The signed opening record, which a provider serves before any frame.
    ///
    /// # Examples
    ///
    /// See [`Sender`].
    pub fn log_open(&self) -> LogOpen {
        self.open
    }

    /// Signs the entry of the next segment, sealed with root `segment_root` and `len` bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Policy`] when the entry would break the chain: [`Fault::Len`] for a `len` of 0 or
    /// more than `segment_bytes`, [`Fault::AfterShort`] after a segment below `segment_bytes`, and
    /// [`Fault::TotalLen`] when the file would pass `u64::MAX` bytes. Nothing changes.
    ///
    /// # Examples
    ///
    /// See [`Sender`].
    pub fn seal(&mut self, segment_root: [u8; 32], len: u32) -> Result<Frame, Error> {
        if len == 0 || len > self.open.segment_bytes {
            return Err(Error::Policy(Fault::Len));
        }
        if self.last_short {
            return Err(Error::Policy(Fault::AfterShort));
        }
        // Every full segment is at least 1 MiB, so the total passes u64::MAX after 2^44
        // entries, long before `next` could.
        let total = self
            .total
            .checked_add(u64::from(len))
            .ok_or(Error::Policy(Fault::TotalLen))?;
        let entry = LogEntry {
            seq: self.next,
            segment_root,
            len,
            prev_entry_hash: self.head,
        };
        let frame = Frame {
            entry,
            sig: sig::sign(
                &self.key,
                Label::Entry,
                &self.open.stream_id,
                &entry.encode(),
            ),
        };
        self.head = entry.hash();
        self.next += 1;
        self.total = total;
        self.last_short = len < self.open.segment_bytes;
        Ok(frame)
    }

    /// Signs the close of the stream. `final_root` is the BLAKE3 root of the whole file, which
    /// the caller computes; the total length and the last entry come from the chain.
    ///
    /// # Examples
    ///
    /// See [`Sender`].
    pub fn close(self, final_root: [u8; 32], closed: u64) -> LogClose {
        let mut close = LogClose {
            final_root,
            total_len: self.total,
            closed,
            last_entry_hash: self.head,
            sig: [0; 64],
        };
        close.sig = sig::sign(
            &self.key,
            Label::Close,
            &self.open.stream_id,
            &close.signed(),
        );
        close
    }
}

/// The key stays out of `Debug`.
impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("next", &self.next)
            .field("total", &self.total)
            .finish_non_exhaustive()
    }
}
