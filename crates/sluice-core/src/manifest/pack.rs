//! Small-file packs, `SPEC.md` section 9: where each file goes, and the bytes of a pack split back
//! into files.
//!
//! [`layout`] is the packing algorithm on sizes alone, so it runs without a disk and the
//! `packs/` vectors can test 64 MiB boundaries with no 64 MiB of bytes. The builder places each
//! file with the same planner as it reads it. [`Unpacker`] is the receiver's side.

mod unpack;

pub use unpack::{Run, Unpacker};

/// A file smaller than this, and not empty, goes into a pack. Fixed by `SPEC.md` section 9, so the
/// same folder gives the same packs on every machine.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_core::manifest::pack::PACK_THRESHOLD, 1 << 20);
/// ```
pub const PACK_THRESHOLD: u64 = 1_048_576;

/// The most bytes a pack holds. Fixed by `SPEC.md` section 9.
///
/// # Examples
///
/// ```
/// assert_eq!(sluice_core::manifest::pack::PACK_TARGET, 64 << 20);
/// ```
pub const PACK_TARGET: u64 = 67_108_864;

/// Where the packing algorithm puts one entry.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::pack::Place;
///
/// let first = Place::Packed { pack_index: 0, offset: 0, len: 10 };
/// assert_ne!(first, Place::Blob);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// Not a file, so no bytes.
    None,
    /// A blob of its own: a file of 0 bytes, or of `PACK_THRESHOLD` bytes or more.
    Blob,
    /// A range of a pack.
    Packed {
        /// The index of the pack.
        pack_index: u32,
        /// Where the file starts in the pack.
        offset: u64,
        /// The size of the file.
        len: u64,
    },
}

/// What [`layout`] gives: one place per entry, in the order given, and the size of each pack.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::pack::{layout, Place};
///
/// let out = layout(&[Some(3), None, Some(4)]);
/// assert_eq!(out.places[1], Place::None);
/// assert_eq!(out.pack_sizes, [7]);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// The place of each entry.
    pub places: Vec<Place>,
    /// The size of each pack, in order of index. Every pack has at least one byte.
    pub pack_sizes: Vec<u64>,
}

/// The packing algorithm of `SPEC.md` section 9 over the entries of a manifest, in manifest order.
/// `Some(size)` is a file of that size and `None` is a folder or a symlink.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::pack::{layout, Place, PACK_THRESHOLD};
///
/// let out = layout(&[Some(0), Some(100), Some(PACK_THRESHOLD), Some(50)]);
/// assert_eq!(
///     out.places,
///     [
///         Place::Blob,
///         Place::Packed { pack_index: 0, offset: 0, len: 100 },
///         Place::Blob,
///         Place::Packed { pack_index: 0, offset: 100, len: 50 },
///     ]
/// );
/// assert_eq!(out.pack_sizes, [150]);
/// ```
pub fn layout(sizes: &[Option<u64>]) -> Layout {
    let mut planner = Planner::default();
    let places = sizes
        .iter()
        .map(|size| size.map_or(Place::None, |size| planner.place(size)))
        .collect();
    Layout {
        places,
        pack_sizes: planner.finish(),
    }
}

/// The algorithm one file at a time, for a builder that learns each size as it opens the file.
#[derive(Default)]
pub(super) struct Planner {
    /// The sizes of the packs that are closed.
    closed: Vec<u64>,
    /// The size of the open pack, whose index is `closed.len()`.
    open: u64,
}

impl Planner {
    /// Steps 2 to 5 of the algorithm for a file of `size` bytes.
    pub(super) fn place(&mut self, size: u64) -> Place {
        if size == 0 || size >= PACK_THRESHOLD {
            return Place::Blob;
        }
        // `open` and `size` are each under PACK_TARGET, so the sum cannot wrap.
        if self.open > 0 && self.open + size > PACK_TARGET {
            self.closed.push(self.open);
            self.open = 0;
        }
        let place = Place::Packed {
            pack_index: self.index(),
            offset: self.open,
            len: size,
        };
        self.open += size;
        place
    }

    /// Step 6: the open pack is closed if it has bytes.
    pub(super) fn finish(mut self) -> Vec<u64> {
        if self.open > 0 {
            self.closed.push(self.open);
        }
        self.closed
    }

    fn index(&self) -> u32 {
        // A pack holds at least one file, so more than u32::MAX packs would take more files than
        // a manifest within any entry limit a machine can hold in memory.
        u32::try_from(self.closed.len()).expect("fewer than 2^32 packs")
    }
}

#[cfg(test)]
mod tests;
