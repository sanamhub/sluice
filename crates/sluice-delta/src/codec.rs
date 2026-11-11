//! The two messages of `SPEC.md` section 7 as bytes: the chunk signatures a provider sends, and the
//! target ranges a requester still needs.
//!
//! Both are length-prefixed frames on the `sluice/1` connection, so a decoder is handed one message
//! and refuses bytes left over. Every count is read before anything is allocated, and every length
//! is checked against the parameters before it is believed: a hint from a peer may be hostile, and
//! the only thing it is allowed to cost is time.

use crate::{Error, chunker::Params};

/// The most entries one message carries. A connection's frame limit bounds a message as well; this
/// is the bound a decoder applies before it allocates, and it is a rule of `SPEC.md` section 7.
pub const MAX_ENTRIES: u32 = 1 << 20;

/// The bytes of one `SigEntry`: the length in four, the strong hash in sixteen.
const ENTRY_BYTES: usize = 20;

/// The bytes of one range: the start in eight, the length in eight.
const RANGE_BYTES: usize = 16;

/// One chunk of the provider's content: how long it is, and the first 16 bytes of its BLAKE3 hash.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    len: u32,
    strong: [u8; 16],
}

/// A chunk is never empty, so there is no is_empty beside len.
#[allow(clippy::len_without_is_empty)]
impl Entry {
    /// An entry for a chunk of `len` bytes whose BLAKE3 hash starts with `strong`. A length of zero
    /// is refused: a chunk has bytes.
    pub fn new(len: u32, strong: [u8; 16]) -> Result<Self, Error> {
        if len == 0 {
            return Err(Error::ChunkLength);
        }
        Ok(Self { len, strong })
    }

    /// The entry of one chunk of content: its length and the first 16 bytes of its BLAKE3 hash.
    ///
    /// This is how a provider builds the list it serves, and how a requester prepares the chunks of
    /// its own file, so both sides hash a chunk the same way.
    pub fn of_bytes(chunk: &[u8]) -> Result<Self, Error> {
        let len = u32::try_from(chunk.len()).map_err(|_| Error::ChunkLength)?;
        let digest = blake3::hash(chunk);
        let strong: [u8; 16] = digest.as_bytes()[..16]
            .try_into()
            .expect("a BLAKE3 digest is 32 bytes");
        Self::new(len, strong)
    }

    /// The chunk's length in bytes.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// The first 16 bytes of the chunk's BLAKE3 hash. Sixteen and not thirty-two, so a peer that
    /// wants two chunks to collide can make them collide on purpose; the root is what decides.
    pub fn strong(&self) -> &[u8; 16] {
        &self.strong
    }
}

/// The chunk signatures of one piece of content, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigList {
    avg_bytes: u32,
    entries: Vec<Entry>,
}

impl SigList {
    /// The only algorithm this release reads and writes: FastCDC v2020 with a 64-bit gear hash.
    pub const ALGO: u8 = 1;

    /// A list for these parameters. The average MUST be one [`Params`] accepts, every chunk MUST be
    /// one this algorithm can cut, and the number of entries MUST be at most [`MAX_ENTRIES`].
    pub fn new(avg_bytes: u32, entries: Vec<Entry>) -> Result<Self, Error> {
        let params = Params::new(avg_bytes)?;
        if entries.len() > MAX_ENTRIES as usize {
            return Err(Error::TooManyEntries);
        }
        if entries.iter().any(|entry| entry.len > params.max()) {
            return Err(Error::ChunkLength);
        }
        Ok(Self { avg_bytes, entries })
    }

    /// The average chunk size the list was cut with.
    pub fn avg_bytes(&self) -> u32 {
        self.avg_bytes
    }

    /// The entries, in the order the content has them.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The sum of the chunk lengths, which the receiver compares with the length its ticket names.
    /// A list whose sum is not that length was cut from other content, and the engine refuses it.
    pub fn total_len(&self) -> u64 {
        self.entries.iter().map(|entry| u64::from(entry.len)).sum()
    }

    /// The bytes of `SPEC.md` section 7: the algorithm, the average as a `u32` in four,
    /// the count as a varint, then one 20-byte entry each.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + varint_len(self.entries.len()) + self.bytes());
        out.push(Self::ALGO);
        out.extend_from_slice(&self.avg_bytes.to_le_bytes());
        out.extend_from_slice(&varint(self.entries.len() as u64));
        for entry in &self.entries {
            out.extend_from_slice(&entry.len.to_le_bytes());
            out.extend_from_slice(&entry.strong);
        }
        out
    }

    /// Reads one message. The count is read before anything is allocated, and the message MUST end
    /// exactly where the count says, so one list has one spelling.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut at = 0;
        let algo = take(bytes, &mut at, 1)?[0];
        if algo != Self::ALGO {
            return Err(Error::Algorithm(algo));
        }
        let avg = u32::from_le_bytes(
            take(bytes, &mut at, 4)?
                .try_into()
                .expect("four bytes were taken"),
        );
        let count = take_varint(bytes, &mut at)?;
        if count > u64::from(MAX_ENTRIES) {
            return Err(Error::TooManyEntries);
        }
        let count = usize::try_from(count).map_err(|_| Error::TooManyEntries)?;
        if bytes.len() < at + count * ENTRY_BYTES {
            return Err(Error::Truncated);
        }
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u32::from_le_bytes(
                take(bytes, &mut at, 4)?
                    .try_into()
                    .expect("four bytes were taken"),
            );
            let strong: [u8; 16] = take(bytes, &mut at, 16)?
                .try_into()
                .expect("sixteen bytes were taken");
            entries.push(Entry::new(len, strong)?);
        }
        if at != bytes.len() {
            return Err(Error::Trailing);
        }
        Self::new(avg, entries)
    }

    /// The bytes the entries take, for the capacity of [`Self::encode`].
    fn bytes(&self) -> usize {
        self.entries.len() * ENTRY_BYTES
    }
}

/// One target range a requester still needs: where it starts, and how many bytes it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    start: u64,
    len: u64,
}

/// A range of no bytes is refused, so there is no is_empty beside len.
#[allow(clippy::len_without_is_empty)]
impl Range {
    /// A range of `len` bytes from `start`. A length of zero is refused: an empty range asks for
    /// nothing and every list of them is one the sender would have to reason about for nothing.
    pub fn new(start: u64, len: u64) -> Result<Self, Error> {
        if len == 0 {
            return Err(Error::Ranges);
        }
        start.checked_add(len).ok_or(Error::Overflow)?;
        Ok(Self { start, len })
    }

    /// Where the range starts.
    pub fn start(&self) -> u64 {
        self.start
    }

    /// How many bytes it is.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Where it ends, which is one past its last byte.
    pub fn end(&self) -> u64 {
        self.start + self.len
    }
}

/// The target ranges a requester still needs, under the root they belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedRanges {
    root: [u8; 32],
    ranges: Vec<Range>,
}

impl NeedRanges {
    /// A set of ranges for `root`. The ranges MUST be ascending and MUST not touch or overlap, so
    /// one set of needed bytes has one encoding.
    pub fn new(root: [u8; 32], ranges: Vec<Range>) -> Result<Self, Error> {
        if ranges.len() > MAX_ENTRIES as usize {
            return Err(Error::TooManyEntries);
        }
        if ranges.windows(2).any(|pair| pair[1].start <= pair[0].end()) {
            return Err(Error::Ranges);
        }
        Ok(Self { root, ranges })
    }

    /// The root the ranges belong to, which is the content the requester is reconstructing.
    pub fn root(&self) -> &[u8; 32] {
        &self.root
    }

    /// The ranges, ascending.
    pub fn ranges(&self) -> &[Range] {
        &self.ranges
    }

    /// How many bytes the ranges ask for.
    pub fn wanted(&self) -> u64 {
        self.ranges.iter().map(|range| range.len).sum()
    }

    /// The bytes of `SPEC.md` section 7: the root, the count as a varint, then one 16-byte range
    /// each, the start and the length as `u64`s.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(
            32 + varint_len(self.ranges.len()) + self.ranges.len() * RANGE_BYTES,
        );
        out.extend_from_slice(&self.root);
        out.extend_from_slice(&varint(self.ranges.len() as u64));
        for range in &self.ranges {
            out.extend_from_slice(&range.start.to_le_bytes());
            out.extend_from_slice(&range.len.to_le_bytes());
        }
        out
    }

    /// Reads one message, refusing a count it cannot hold, ranges that are not ascending and apart,
    /// and bytes after the last range.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut at = 0;
        let root: [u8; 32] = take(bytes, &mut at, 32)?
            .try_into()
            .expect("thirty-two bytes were taken");
        let count = take_varint(bytes, &mut at)?;
        if count > u64::from(MAX_ENTRIES) {
            return Err(Error::TooManyEntries);
        }
        let count = usize::try_from(count).map_err(|_| Error::TooManyEntries)?;
        if bytes.len() < at + count * RANGE_BYTES {
            return Err(Error::Truncated);
        }
        let mut ranges = Vec::with_capacity(count);
        for _ in 0..count {
            let start = u64::from_le_bytes(
                take(bytes, &mut at, 8)?
                    .try_into()
                    .expect("eight bytes were taken"),
            );
            let len = u64::from_le_bytes(
                take(bytes, &mut at, 8)?
                    .try_into()
                    .expect("eight bytes were taken"),
            );
            ranges.push(Range::new(start, len)?);
        }
        if at != bytes.len() {
            return Err(Error::Trailing);
        }
        Self::new(root, ranges)
    }
}

/// The shortest LEB128 spelling of `value`, which is what a count is written as.
fn varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return out;
        }
        out.push(low | 0x80);
    }
}

/// How many bytes [`varint`] writes for `value`.
fn varint_len(value: usize) -> usize {
    let mut value = value as u64;
    let mut len = 1;
    while value >= 0x80 {
        value >>= 7;
        len += 1;
    }
    len
}

/// The next `len` bytes, or [`Error::Truncated`].
fn take<'a>(bytes: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8], Error> {
    let end = at.checked_add(len).ok_or(Error::Truncated)?;
    let slice = bytes.get(*at..end).ok_or(Error::Truncated)?;
    *at = end;
    Ok(slice)
}

/// A count: the shortest LEB128 spelling of a value, and only that one, so one number has one
/// encoding. A count past `u64` and a longer spelling are both refused.
fn take_varint(bytes: &[u8], at: &mut usize) -> Result<u64, Error> {
    let mut value = 0u64;
    for count in 1..=10u32 {
        let byte = take(bytes, at, 1)?[0];
        let bits = u64::from(byte & 0x7f);
        if count == 10 && bits > 1 {
            return Err(Error::Varint);
        }
        value |= bits << (7 * (count - 1));
        if byte & 0x80 == 0 {
            if count > 1 && bits == 0 {
                return Err(Error::Varint);
            }
            return Ok(value);
        }
    }
    Err(Error::Varint)
}

#[cfg(test)]
mod tests;
