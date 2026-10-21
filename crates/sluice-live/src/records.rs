//! The four records of `SPEC.md` section 6, in their fixed little-endian layouts.
//!
//! Every field has a fixed width and there is no varint, so any byte string of the right length
//! is exactly one record and a record has exactly one spelling. A decoder has nothing to check but
//! the length; what the fields mean is the receiver's to check.

use crate::{Error, Fault};

/// The record a stream starts with: who signs it, and how big a full segment is.
///
/// # Examples
///
/// ```
/// use sluice_live::LogOpen;
///
/// let open = LogOpen {
///     stream_id: [1; 16],
///     sender_key: [2; 32],
///     segment_bytes: 8 << 20,
///     started: 1_800_000_000_000,
///     sig: [0; 64],
/// };
/// let bytes = open.encode();
/// assert_eq!(bytes.len(), LogOpen::LEN);
/// assert_eq!(LogOpen::decode(&bytes), Ok(open));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogOpen {
    /// The stream the ticket names.
    pub stream_id: [u8; 16],
    /// The Ed25519 public key that signs every record of the stream.
    pub sender_key: [u8; 32],
    /// The size of a full segment.
    pub segment_bytes: u32,
    /// UTC milliseconds when the stream began. Signed, and checked against nothing.
    pub started: u64,
    /// The sender's signature over the first 60 bytes.
    pub sig: [u8; 64],
}

/// One sealed segment: its place in the chain, its root and its length.
///
/// # Examples
///
/// ```
/// use sluice_live::LogEntry;
///
/// let entry = LogEntry { seq: 0, segment_root: [3; 32], len: 5, prev_entry_hash: [4; 32] };
/// assert_eq!(LogEntry::decode(&entry.encode()), Ok(entry));
/// assert_eq!(entry.hash(), *blake3::hash(&entry.encode()).as_bytes());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// The position in the chain, from 0.
    pub seq: u64,
    /// The BLAKE3 root of the segment's bytes.
    pub segment_root: [u8; 32],
    /// The number of bytes in the segment.
    pub len: u32,
    /// The hash of the entry before, or of the `LogOpen` for `seq` 0.
    pub prev_entry_hash: [u8; 32],
}

/// A `LogEntry` and the sender's signature over it.
///
/// # Examples
///
/// ```
/// use sluice_live::{Frame, LogEntry};
///
/// let entry = LogEntry { seq: 7, segment_root: [3; 32], len: 5, prev_entry_hash: [4; 32] };
/// let frame = Frame { entry, sig: [9; 64] };
/// let bytes = frame.encode();
/// assert_eq!(bytes[..LogEntry::LEN], entry.encode());
/// assert_eq!(Frame::decode(&bytes), Ok(frame));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The entry that is signed.
    pub entry: LogEntry,
    /// The sender's signature over the 76 bytes of `entry`.
    pub sig: [u8; 64],
}

/// The record a stream ends with: the whole file's root and length, and the last entry.
///
/// # Examples
///
/// ```
/// use sluice_live::LogClose;
///
/// let close = LogClose {
///     final_root: [5; 32],
///     total_len: 12,
///     closed: 1_800_000_000_001,
///     last_entry_hash: [6; 32],
///     sig: [0; 64],
/// };
/// assert_eq!(LogClose::decode(&close.encode()), Ok(close));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogClose {
    /// The BLAKE3 root of the whole file, which the caller checks.
    pub final_root: [u8; 32],
    /// The length of the whole file, the sum of every entry's `len`.
    pub total_len: u64,
    /// UTC milliseconds when the stream ended. Signed, and checked against nothing.
    pub closed: u64,
    /// The hash of the last entry, or of the `LogOpen` when there is none.
    pub last_entry_hash: [u8; 32],
    /// The sender's signature over the first 80 bytes.
    pub sig: [u8; 64],
}

/// Writes fields end to end into a record of known length.
struct Writer<const N: usize> {
    out: [u8; N],
    at: usize,
}

impl<const N: usize> Writer<N> {
    fn new() -> Self {
        Self { out: [0; N], at: 0 }
    }

    fn put(mut self, field: &[u8]) -> Self {
        self.out[self.at..self.at + field.len()].copy_from_slice(field);
        self.at += field.len();
        self
    }

    fn done(self) -> [u8; N] {
        debug_assert_eq!(self.at, N, "the fields fill the record");
        self.out
    }
}

/// Reads fields end to end from a record whose length was checked.
struct Reader<'a> {
    rest: &'a [u8],
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let (field, rest) = self.rest.split_at(N);
        self.rest = rest;
        field.try_into().expect("split_at gave N bytes")
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }
}

fn reader<const N: usize>(bytes: &[u8]) -> Result<Reader<'_>, Error> {
    if bytes.len() == N {
        Ok(Reader { rest: bytes })
    } else {
        Err(Error::Verify(Fault::Length))
    }
}

impl LogOpen {
    /// The length of the encoding.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(sluice_live::LogOpen::LEN, 124);
    /// ```
    pub const LEN: usize = 124;
    /// The length of the part that is signed and hashed, everything before `sig`.
    const SIGNED: usize = 60;

    /// The 124 bytes of the record.
    ///
    /// # Examples
    ///
    /// See [`LogOpen`].
    pub fn encode(&self) -> [u8; Self::LEN] {
        let signed = self.signed();
        Writer::new().put(&signed).put(&self.sig).done()
    }

    /// The record in `bytes`.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] with [`Fault::Length`] unless `bytes` is 124 bytes long.
    ///
    /// # Examples
    ///
    /// See [`LogOpen`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = reader::<{ Self::LEN }>(bytes)?;
        Ok(Self {
            stream_id: r.take(),
            sender_key: r.take(),
            segment_bytes: r.u32(),
            started: r.u64(),
            sig: r.take(),
        })
    }

    /// The BLAKE3 hash of the first 60 bytes: the `prev_entry_hash` of `seq` 0, and the
    /// `last_entry_hash` of a stream with no entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_live::LogOpen;
    ///
    /// let open = LogOpen::decode(&[0; LogOpen::LEN])?;
    /// assert_eq!(open.hash(), *blake3::hash(&[0; 60]).as_bytes());
    /// # Ok::<(), sluice_live::Error>(())
    /// ```
    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.signed()).as_bytes()
    }

    pub(crate) fn signed(&self) -> [u8; Self::SIGNED] {
        Writer::new()
            .put(&self.stream_id)
            .put(&self.sender_key)
            .put(&self.segment_bytes.to_le_bytes())
            .put(&self.started.to_le_bytes())
            .done()
    }
}

impl LogEntry {
    /// The length of the encoding.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(sluice_live::LogEntry::LEN, 76);
    /// ```
    pub const LEN: usize = 76;

    /// The 76 bytes of the entry, which its signature covers.
    ///
    /// # Examples
    ///
    /// See [`LogEntry`].
    pub fn encode(&self) -> [u8; Self::LEN] {
        Writer::new()
            .put(&self.seq.to_le_bytes())
            .put(&self.segment_root)
            .put(&self.len.to_le_bytes())
            .put(&self.prev_entry_hash)
            .done()
    }

    /// The entry in `bytes`.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] with [`Fault::Length`] unless `bytes` is 76 bytes long.
    ///
    /// # Examples
    ///
    /// See [`LogEntry`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = reader::<{ Self::LEN }>(bytes)?;
        Ok(Self {
            seq: r.u64(),
            segment_root: r.take(),
            len: r.u32(),
            prev_entry_hash: r.take(),
        })
    }

    /// The BLAKE3 hash of the 76 bytes, which the next entry names as its `prev_entry_hash`.
    ///
    /// # Examples
    ///
    /// See [`LogEntry`].
    pub fn hash(&self) -> [u8; 32] {
        *blake3::hash(&self.encode()).as_bytes()
    }
}

impl Frame {
    /// The length of the encoding.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(sluice_live::Frame::LEN, 140);
    /// ```
    pub const LEN: usize = 140;

    /// The 140 bytes of the frame: the entry, then the signature.
    ///
    /// # Examples
    ///
    /// See [`Frame`].
    pub fn encode(&self) -> [u8; Self::LEN] {
        Writer::new()
            .put(&self.entry.encode())
            .put(&self.sig)
            .done()
    }

    /// The frame in `bytes`.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] with [`Fault::Length`] unless `bytes` is 140 bytes long.
    ///
    /// # Examples
    ///
    /// See [`Frame`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = reader::<{ Self::LEN }>(bytes)?;
        let entry: [u8; LogEntry::LEN] = r.take();
        Ok(Self {
            entry: LogEntry::decode(&entry)?,
            sig: r.take(),
        })
    }
}

impl LogClose {
    /// The length of the encoding.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(sluice_live::LogClose::LEN, 144);
    /// ```
    pub const LEN: usize = 144;
    const SIGNED: usize = 80;

    /// The 144 bytes of the record.
    ///
    /// # Examples
    ///
    /// See [`LogClose`].
    pub fn encode(&self) -> [u8; Self::LEN] {
        let signed = self.signed();
        Writer::new().put(&signed).put(&self.sig).done()
    }

    /// The record in `bytes`.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] with [`Fault::Length`] unless `bytes` is 144 bytes long.
    ///
    /// # Examples
    ///
    /// See [`LogClose`].
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = reader::<{ Self::LEN }>(bytes)?;
        Ok(Self {
            final_root: r.take(),
            total_len: r.u64(),
            closed: r.u64(),
            last_entry_hash: r.take(),
            sig: r.take(),
        })
    }

    pub(crate) fn signed(&self) -> [u8; Self::SIGNED] {
        Writer::new()
            .put(&self.final_root)
            .put(&self.total_len.to_le_bytes())
            .put(&self.closed.to_le_bytes())
            .put(&self.last_entry_hash)
            .done()
    }
}
