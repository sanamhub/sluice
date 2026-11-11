//! Content-defined chunking with a normalized gear hash in the FastCDC style, as `SPEC.md` section
//! 7 states it: not the FastCDC v2020 tool's cut, and the section is what an implementation follows.
//!
//! A chunk boundary depends on the bytes just before it and on nothing else, so inserting a byte at
//! the front of a file moves one boundary's worth of data instead of all of it. That is the whole
//! point of the algorithm here: a resend of a file that gained a paragraph should fetch the
//! paragraph, not the file.

use std::sync::OnceLock;

use crate::Error;

/// The smallest average this release writes or reads: 256 bytes, which gives a grain of 64 bytes.
pub const MIN_AVG: u32 = 256;
/// The largest average: 4 MiB, which gives a chunk of at most 32 MiB.
pub const MAX_AVG: u32 = 4 << 20;
/// The file size below which a delta costs more than it saves. `SPEC.md` section 7 states it, and
/// the engine uses it to decide whether to ask for one.
pub const SMALLEST: u64 = 64 << 10;

/// The label the gear table is derived from, one BLAKE3 call per entry.
const GEAR_LABEL: &[u8] = b"sluice/delta/gear/1";

/// One set of chunking parameters: an average that is a power of two, and the minimum and maximum
/// that follow from it. `min = avg / 4` and `max = avg * 8`, so two implementations that read the
/// same `avg_bytes` from a `SigList` cut the same bytes at the same places.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    avg: u32,
    min: u32,
    max: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self::new(crate::AVG_BYTES).expect("8 KiB is a power of two within the bounds")
    }
}

impl Params {
    /// The parameters for `avg_bytes`, which MUST be a power of two from [`MIN_AVG`] to
    /// [`MAX_AVG`], both included.
    pub fn new(avg_bytes: u32) -> Result<Self, Error> {
        if !(MIN_AVG..=MAX_AVG).contains(&avg_bytes) || !avg_bytes.is_power_of_two() {
            return Err(Error::Average);
        }
        Ok(Self {
            avg: avg_bytes,
            min: avg_bytes / 4,
            max: avg_bytes * 8,
        })
    }

    /// The average this set cuts at.
    pub fn avg(&self) -> u32 {
        self.avg
    }

    /// The shortest chunk.
    pub fn min(&self) -> u32 {
        self.min
    }

    /// The longest chunk.
    pub fn max(&self) -> u32 {
        self.max
    }

    /// Cuts `data` into chunks. An empty input has none.
    pub fn chunks<'a>(&self, data: &'a [u8]) -> Chunks<'a> {
        Chunks {
            data,
            at: 0,
            params: *self,
        }
    }

    /// The end offsets of the chunks of `data`, in order, which is what a test compares.
    pub fn boundaries(&self, data: &[u8]) -> Vec<usize> {
        let mut boundaries = Vec::new();
        let mut at = 0;
        while at < data.len() {
            at += self.cut(&data[at..]);
            boundaries.push(at);
        }
        boundaries
    }

    /// The length of the first chunk of `bytes`.
    fn cut(&self, bytes: &[u8]) -> usize {
        let (min, avg, max) = (self.min as usize, self.avg as usize, self.max as usize);
        if bytes.len() <= min {
            return bytes.len();
        }
        // The gear hash is the bytes' own, not the chunk's: a boundary depends on the bytes before
        // it and nothing else, so the first `min` bytes are skipped and the hash starts at zero.
        let mut hash = 0u64;
        let center = avg.min(bytes.len());
        let (before, after) = (self.mask(2), self.mask(-2));
        for (at, byte) in bytes.iter().enumerate().take(center).skip(min) {
            hash = (hash << 1).wrapping_add(gear(*byte));
            if hash & before == 0 {
                return at + 1;
            }
        }
        let end = max.min(bytes.len());
        for (at, byte) in bytes.iter().enumerate().take(end).skip(center) {
            hash = (hash << 1).wrapping_add(gear(*byte));
            if hash & after == 0 {
                return at + 1;
            }
        }
        end
    }

    /// The mask one region of the scan cuts on: two more bits than the average before it, two fewer
    /// after, which pulls boundaries towards the average without letting them drift.
    fn mask(&self, shift: i32) -> u64 {
        let bits = i32::try_from(self.avg.trailing_zeros()).unwrap_or(32) + shift;
        let bits = u32::try_from(bits).unwrap_or(0).min(63);
        (1u64 << bits) - 1
    }
}

/// The chunks of one input.
pub struct Chunks<'a> {
    data: &'a [u8],
    at: usize,
    params: Params,
}

impl<'a> Iterator for Chunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.at >= self.data.len() {
            return None;
        }
        let len = self.params.cut(&self.data[self.at..]);
        let chunk = &self.data[self.at..self.at + len];
        self.at += len;
        Some(chunk)
    }
}

/// The gear table of `SPEC.md` section 7: entry `i` is the first eight bytes, little-endian, of
/// `BLAKE3("sluice/delta/gear/1" || [i])`. Stating the table as a hash instead of 256 constants
/// means an implementation cannot transcribe it wrongly, and the table is the same on every machine
/// and in every language.
fn gear(byte: u8) -> u64 {
    static GEAR: OnceLock<[u64; 256]> = OnceLock::new();
    GEAR.get_or_init(|| {
        let mut table = [0u64; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut hasher = blake3::Hasher::new();
            hasher.update(GEAR_LABEL);
            hasher.update(&[index as u8]);
            let digest = hasher.finalize();
            let head: [u8; 8] = digest.as_bytes()[..8]
                .try_into()
                .expect("a BLAKE3 digest is 32 bytes");
            *slot = u64::from_le_bytes(head);
        }
        table
    })[usize::from(byte)]
}

#[cfg(test)]
mod tests;
