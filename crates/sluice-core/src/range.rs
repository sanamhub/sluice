//! Byte ranges of a file, as `sluice recv --range 4G:4.1G` takes them, and the blocks that fetch
//! one.
//!
//! A range is half-open, like a Rust range: `a:b` is from byte `a` up to but not including `b`.
//! It is parsed on its own, then resolved against the size of the file, then planned as whole
//! blocks of `iroh-blobs`, because a range is only verified a block at a time. A writer streams
//! the verified blocks and keeps exactly the bytes of the range, so the output holds those bytes
//! and nothing else.

mod size;
#[cfg(test)]
mod tests;

use std::ops::Range;

pub use size::ByteSize;

use crate::Error;

/// The block `iroh-blobs` verifies at a time: a chunk group of 16 chunks of 1 KiB. It is a
/// constant there, `IROH_BLOCK_SIZE`, and so it is one here.
///
/// # Examples
///
/// ```
/// use sluice_core::range::{BLOCK_BYTES, ByteRange};
///
/// let plan = ByteRange::parse(":1")?.resolve(1 << 20)?.plan();
/// assert_eq!(plan.fetched().end, BLOCK_BYTES);
/// # Ok::<(), sluice_core::Error>(())
/// ```
pub const BLOCK_BYTES: u64 = 16 * 1024;

/// `Error::Policy` for text that is not a size or a range. The text is the user's own, not a file
/// name or a ticket, so it is quoted to show what was read. `{:?}` escapes what would not print.
fn refuse(text: &str, why: &str) -> Error {
    Error::Policy(format!(
        "{text:?} is not a number of bytes or a byte range: {why}"
    ))
}

/// A range of bytes as the user wrote it, before the file's size is known.
///
/// # Examples
///
/// ```
/// use sluice_core::range::ByteRange;
///
/// let range = ByteRange::parse("4G:4.1G")?;
/// assert_eq!((range.start(), range.end()), (4 << 30, Some(4_402_341_478)));
/// assert_eq!(ByteRange::parse("1K:+1K")?.end(), Some(2048));
/// assert_eq!(ByteRange::parse("100:")?.end(), None);
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    start: u64,
    end: Option<u64>,
}

impl ByteRange {
    /// Reads one of four forms, each side a size as [`ByteSize::parse`] reads it:
    ///
    /// - `a:b`, from `a` up to but not including `b`
    /// - `a:`, from `a` to the end of the file
    /// - `:b`, from the start of the file up to `b`
    /// - `a:+n`, the `n` bytes from `a`
    ///
    /// Fails with [`Error::Policy`], quoting the text, for an empty range (`a:a` or `a:+0`), a
    /// start after its end, `:` alone, an end past `u64::MAX`, or a side that is not a size.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse(":512")?.start(), 0);
    /// for bad in ["", ":", "5:5", "5:+0", "6:5", "5", "5:+-1", "1:2:3"] {
    ///     assert!(ByteRange::parse(bad).is_err(), "{bad}");
    /// }
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self, Error> {
        let Some((from, to)) = text.split_once(':') else {
            return Err(refuse(
                text,
                "a range is a start and an end with a colon between",
            ));
        };
        if from.is_empty() && to.is_empty() {
            return Err(refuse(text, "it gives neither a start nor an end"));
        }
        let start = if from.is_empty() {
            0
        } else {
            size::parse_in(from, text)?
        };
        let end = if to.is_empty() {
            None
        } else if let Some(length) = to.strip_prefix('+').filter(|_| !from.is_empty()) {
            let length = size::parse_in(length, text)?;
            Some(
                start
                    .checked_add(length)
                    .ok_or_else(|| refuse(text, "it ends past u64::MAX bytes"))?,
            )
        } else {
            Some(size::parse_in(to, text)?)
        };
        match end {
            Some(end) if end == start => Err(refuse(text, "it is empty")),
            Some(end) if end < start => Err(refuse(text, "it starts after its end")),
            _ => Ok(Self { start, end }),
        }
    }

    /// The first byte of the range.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("2K:")?.start(), 2048);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn start(&self) -> u64 {
        self.start
    }

    /// The byte after the last one, or `None` for a range that runs to the end of the file.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse(":2K")?.end(), Some(2048));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn end(&self) -> Option<u64> {
        self.end
    }

    /// The range in a file of `file_size` bytes. An end past the size is cut to the size, as an
    /// HTTP `Range` is, and [`Resolved::cut_from`] says so. A start at or past the size fails with
    /// [`Error::Policy`], because no byte of the file is in the range.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// let resolved = ByteRange::parse("1K:1M")?.resolve(4096)?;
    /// assert_eq!(resolved.bytes(), 1024..4096);
    /// assert_eq!(resolved.cut_from(), Some(1 << 20));
    /// assert!(ByteRange::parse("4K:")?.resolve(4096).is_err());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn resolve(&self, file_size: u64) -> Result<Resolved, Error> {
        if self.start >= file_size {
            return Err(Error::Policy(format!(
                "the range starts at byte {}, and the file has {file_size} bytes, so no byte of it \
                 is in the range",
                self.start
            )));
        }
        let end = self.end.unwrap_or(file_size);
        Ok(Resolved {
            start: self.start,
            end: end.min(file_size),
            file_size,
            cut_from: self.end.filter(|end| *end > file_size),
        })
    }
}

/// A range that holds at least one byte of a file whose size is known.
///
/// # Examples
///
/// ```
/// use sluice_core::range::ByteRange;
///
/// let resolved = ByteRange::parse("10:20")?.resolve(100)?;
/// assert_eq!((resolved.bytes(), resolved.file_size(), resolved.cut_from()), (10..20, 100, None));
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    start: u64,
    end: u64,
    file_size: u64,
    cut_from: Option<u64>,
}

impl Resolved {
    /// The bytes of the range, never empty and never past the end of the file.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("5:")?.resolve(8)?.bytes(), 5..8);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn bytes(&self) -> Range<u64> {
        self.start..self.end
    }

    /// The size of the file the range was resolved against.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("5:")?.resolve(8)?.file_size(), 8);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn file_size(&self) -> u64 {
        self.file_size
    }

    /// The end the user asked for, when it was past the end of the file and the range was cut to
    /// the file's size. A command says so, so that the user is not left to wonder why the output
    /// is shorter than the range they wrote.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("0:9")?.resolve(8)?.cut_from(), Some(9));
    /// assert_eq!(ByteRange::parse("0:8")?.resolve(8)?.cut_from(), None);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn cut_from(&self) -> Option<u64> {
        self.cut_from
    }

    /// The blocks to fetch for this range, and which of their bytes to keep.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// // Bytes 20,000 to 40,000 of a 1 MiB file lie in blocks 1 and 2.
    /// let plan = ByteRange::parse("20000:40000")?.resolve(1 << 20)?.plan();
    /// assert_eq!(plan.blocks(), 1..3);
    /// assert_eq!((plan.skip(), plan.keep()), (20_000 - 16_384, 20_000));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn plan(&self) -> FetchPlan {
        let first = self.start / BLOCK_BYTES;
        // `end` is above `start`, so `end - 1` is the last byte and there is one.
        let last = (self.end - 1) / BLOCK_BYTES;
        FetchPlan {
            blocks: first..last + 1,
            skip: self.start - first * BLOCK_BYTES,
            keep: self.end - self.start,
            file_size: self.file_size,
        }
    }
}

/// What to fetch for a range: whole blocks of [`BLOCK_BYTES`], from the one that holds the first
/// byte to the one that holds the last. Each block is verified against the root of the whole file
/// as it comes in. The writer drops [`FetchPlan::skip`] bytes at the front of the first block and
/// keeps the next [`FetchPlan::keep`].
///
/// # Examples
///
/// ```
/// use sluice_core::range::ByteRange;
///
/// // The last 100 bytes of a file of 40,000: the third block, which is short.
/// let plan = ByteRange::parse("39900:")?.resolve(40_000)?.plan();
/// assert_eq!(plan.blocks(), 2..3);
/// assert_eq!(plan.fetched(), 32_768..40_000);
/// assert_eq!((plan.skip(), plan.keep()), (39_900 - 32_768, 100));
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchPlan {
    blocks: Range<u64>,
    skip: u64,
    keep: u64,
    file_size: u64,
}

impl FetchPlan {
    /// The blocks to fetch, by number: block `i` holds bytes `i * 16384` up to `(i + 1) * 16384`,
    /// or to the end of the file for the last one.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse(":16K")?.resolve(1 << 20)?.plan().blocks(), 0..1);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn blocks(&self) -> Range<u64> {
        self.blocks.clone()
    }

    /// The bytes those blocks hold. The last block of a file can be short, so this ends at the
    /// file's size if the blocks reach it.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("0:1")?.resolve(10)?.plan().fetched(), 0..10);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn fetched(&self) -> Range<u64> {
        let start = self.blocks.start * BLOCK_BYTES;
        let end = self
            .blocks
            .end
            .saturating_mul(BLOCK_BYTES)
            .min(self.file_size);
        start..end
    }

    /// How many bytes at the front of the first block come before the range.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("16385:+1")?.resolve(1 << 20)?.plan().skip(), 1);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn skip(&self) -> u64 {
        self.skip
    }

    /// How many bytes to keep after the skipped ones: the length of the range.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteRange;
    ///
    /// assert_eq!(ByteRange::parse("16385:+1")?.resolve(1 << 20)?.plan().keep(), 1);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn keep(&self) -> u64 {
        self.keep
    }
}
