//! The receiver's side of a pack: which file each byte belongs to, while the pack streams in.
//!
//! It is sans-IO. It is given the bytes in pieces of any size and says, for each run of bytes in a
//! piece, whose they are and whether the file ends there. It copies nothing and keeps no byte, so
//! a whole pack is never in memory. Writing the files is the writer's work.

use std::ops::Range;

use crate::Error;
use crate::manifest::{Loc, Manifest};

/// A run of bytes of one piece that belongs to one file.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::pack::Run;
///
/// let run = Run { entry: 3, bytes: 0..10, ends: true };
/// assert_eq!(run.bytes.len(), 10);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    /// The index of the file in `manifest.entries()`.
    pub entry: usize,
    /// Where the run is in the piece that was given. It is empty for a file of no bytes.
    pub bytes: Range<usize>,
    /// The file's last byte is in this run, so the file is complete.
    pub ends: bool,
}

/// Splits one pack of a manifest back into its files.
///
/// The manifest was decoded, so rule 16 holds: the files of the pack lie end to end and fill it,
/// and every byte has exactly one file.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Entry, Kind, Loc, Manifest, PackRef, path::ManifestPath};
/// use sluice_core::manifest::pack::{Run, Unpacker};
///
/// let file = |name: &str, offset: u64, len: u64| -> Result<Entry, sluice_core::Error> {
///     Ok(Entry {
///         path: ManifestPath::parse(name)?,
///         kind: Kind::File,
///         size: len,
///         mtime_ms: 0,
///         mode: 0o644,
///         loc: Loc::Packed { pack_index: 0, offset, len },
///         link_target: None,
///     })
/// };
/// let pack = PackRef { root: *blake3::hash(b"helloworld").as_bytes(), size: 10 };
/// let manifest = Manifest::new(vec![file("a", 0, 5)?, file("b", 5, 5)?], vec![pack])?;
///
/// // The pack is `helloworld`, and it arrives in two pieces that do not end where a file does.
/// let mut unpacker = Unpacker::new(&manifest, 0)?;
/// assert_eq!(
///     unpacker.feed(b"hellowo")?,
///     [
///         Run { entry: 0, bytes: 0..5, ends: true },
///         Run { entry: 1, bytes: 5..7, ends: false },
///     ]
/// );
/// assert_eq!(unpacker.feed(b"rld")?, [Run { entry: 1, bytes: 0..3, ends: true }]);
/// assert!(unpacker.finish()?.is_empty());
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Debug)]
pub struct Unpacker {
    /// The index and size of each file in the pack, in pack order.
    files: Vec<(usize, u64)>,
    /// The size of the pack.
    size: u64,
    /// How many bytes of the pack were given.
    given: u64,
    /// The position in `files` of the file the next byte belongs to.
    next: usize,
    /// How many bytes of that file were given.
    into_next: u64,
}

impl Unpacker {
    /// An unpacker for the pack at `pack_index` of `manifest`.
    ///
    /// # Errors
    ///
    /// [`Error::Policy`] when the manifest has no pack at that index.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{Manifest, pack::Unpacker};
    ///
    /// let empty = Manifest::new(vec![], vec![])?;
    /// assert!(Unpacker::new(&empty, 0).is_err());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn new(manifest: &Manifest, pack_index: u32) -> Result<Self, Error> {
        let pack = usize::try_from(pack_index)
            .ok()
            .and_then(|at| manifest.packs().get(at))
            .ok_or_else(|| Error::Policy(format!("the manifest has no pack {pack_index}")))?;
        // Rule 16 puts the files of a pack in manifest order, so the order of the entries is the
        // order of the bytes.
        let files = manifest
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(at, entry)| match entry.loc {
                Loc::Packed {
                    pack_index: index,
                    len,
                    ..
                } if index == pack_index => Some((at, len)),
                _ => None,
            })
            .collect();
        Ok(Self {
            files,
            size: pack.size,
            given: 0,
            next: 0,
            into_next: 0,
        })
    }

    /// The runs of `piece`, the next bytes of the pack, in order. A file of no bytes that sits
    /// where the pack has got to is a run of its own with an empty range.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] when the piece goes past the size of the pack. Nothing of it is used.
    ///
    /// # Examples
    ///
    /// See [`Unpacker`].
    pub fn feed(&mut self, piece: &[u8]) -> Result<Vec<Run>, Error> {
        if piece.len() as u64 > self.size - self.given {
            return Err(Error::Verify(format!(
                "a pack of {} bytes gave more than that",
                self.size
            )));
        }
        let mut runs = Vec::new();
        let mut at = 0;
        loop {
            self.empty_files(at, &mut runs);
            let Some(&(entry, len)) = self.files.get(self.next) else {
                break;
            };
            if at == piece.len() {
                break;
            }
            let left = len - self.into_next;
            // Under the piece's length, so it fits a usize.
            let take = left.min((piece.len() - at) as u64) as usize;
            let ends = take as u64 == left;
            runs.push(Run {
                entry,
                bytes: at..at + take,
                ends,
            });
            at += take;
            self.given += take as u64;
            self.into_next += take as u64;
            if ends {
                self.next += 1;
                self.into_next = 0;
            }
        }
        Ok(runs)
    }

    /// Ends the pack. It returns the files of no bytes that no piece reached, which happens only
    /// for a pack of no bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] when the pack gave fewer bytes than its size.
    ///
    /// # Examples
    ///
    /// See [`Unpacker`].
    pub fn finish(mut self) -> Result<Vec<Run>, Error> {
        if self.given != self.size {
            return Err(Error::Verify(format!(
                "a pack of {} bytes ended after {}",
                self.size, self.given
            )));
        }
        let mut runs = Vec::new();
        self.empty_files(0, &mut runs);
        Ok(runs)
    }

    /// The files of no bytes that start where the pack has got to. They are complete already.
    fn empty_files(&mut self, at: usize, runs: &mut Vec<Run>) {
        while let Some(&(entry, 0)) = self.files.get(self.next) {
            runs.push(Run {
                entry,
                bytes: at..at,
                ends: true,
            });
            self.next += 1;
        }
    }
}
