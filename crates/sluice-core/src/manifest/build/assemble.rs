//! The entries of a walked folder in manifest order, each file read either into a blob of its own
//! or into its pack, as `SPEC.md` section 9 places it.
//!
//! The size that decides between the two comes from the open handle, so the file that is placed
//! is the file that is read. A pack is hashed as its files are read, one after another, and is
//! never written to disk or held in memory.

use std::path::{Path, PathBuf};

use super::hash::{self, Facts, HashFault};
use super::walk::{Item, What};
use super::{Finding, Reason};
use crate::manifest::entry::{Entry, Kind, Loc, PackRef};
use crate::manifest::pack::{Place, Planner};
use crate::manifest::path::ManifestPath;

/// What the items came to, in manifest order.
pub(super) struct Assembled {
    pub entries: Vec<Entry>,
    /// The disk path of each entry that is a file, at the entry's index.
    pub disk: Vec<Option<PathBuf>>,
    pub packs: Vec<PackRef>,
    /// Files that could not be read. When there is one, the rest is not a manifest to send.
    pub problems: Vec<Finding>,
}

/// A file changed while it was read, so no hash names a version of it.
pub(super) struct Changed;

/// Reads every item, which must be sorted by path. `between` runs for each file after its last
/// byte is read and before the second look at it, which is where a test changes the file.
pub(super) fn assemble(items: Vec<Item>, between: &impl Fn(&Path)) -> Result<Assembled, Changed> {
    let mut packs = Packs::default();
    let mut out = Assembled {
        entries: Vec::with_capacity(items.len()),
        disk: Vec::with_capacity(items.len()),
        packs: Vec::new(),
        problems: Vec::new(),
    };
    for Item { path, shown, what } in items {
        let (entry, disk) = match what {
            What::Dir { mtime_ms, mode } => (no_bytes(path, Kind::Dir, mtime_ms, mode), None),
            What::Symlink { target, mtime_ms } => {
                let entry = Entry {
                    link_target: Some(target),
                    ..no_bytes(path, Kind::Symlink, mtime_ms, 0)
                };
                (entry, None)
            }
            What::File { disk } => match packs.read(&disk, between) {
                Ok((facts, loc)) => (file(path, &facts, loc), Some(disk)),
                Err(HashFault::Unreadable(kind)) => {
                    // Listed, and the walk goes on, so every such file is found in one run.
                    let reason = Reason::Unreadable(kind);
                    out.problems.push(Finding {
                        path: shown,
                        reason,
                    });
                    continue;
                }
                Err(HashFault::Changed) => return Err(Changed),
            },
        };
        out.entries.push(entry);
        out.disk.push(disk);
    }
    out.packs = packs.finish();
    Ok(out)
}

/// The planner, and the hasher of the open pack.
#[derive(Default)]
struct Packs {
    planner: Planner,
    open: Option<(u32, blake3::Hasher)>,
    roots: Vec<[u8; 32]>,
}

impl Packs {
    /// Opens the file, places it by the size of the open handle, and reads it into its hasher.
    fn read(&mut self, disk: &Path, between: &impl Fn(&Path)) -> Result<(Facts, Loc), HashFault> {
        let opened = hash::open(disk)?;
        match self.planner.place(opened.size()) {
            Place::Packed {
                pack_index,
                offset,
                len,
            } => {
                let facts = hash::read_into(opened, self.hasher(pack_index), || between(disk))?;
                let loc = Loc::Packed {
                    pack_index,
                    offset,
                    len,
                };
                Ok((facts, loc))
            }
            Place::Blob | Place::None => {
                let mut hasher = blake3::Hasher::new();
                let facts = hash::read_into(opened, &mut hasher, || between(disk))?;
                let root = *hasher.finalize().as_bytes();
                Ok((facts, Loc::Blob { root }))
            }
        }
    }

    /// The hasher of pack `index`. A new index closes the pack before it, because the planner
    /// only ever moves on to the next pack.
    fn hasher(&mut self, index: u32) -> &mut blake3::Hasher {
        if self.open.as_ref().is_some_and(|(open, _)| *open != index) {
            self.close();
        }
        &mut self
            .open
            .get_or_insert_with(|| (index, blake3::Hasher::new()))
            .1
    }

    fn close(&mut self) {
        if let Some((_, hasher)) = self.open.take() {
            self.roots.push(*hasher.finalize().as_bytes());
        }
    }

    /// The packs, each with the root of its bytes and the size the planner gave it. Every byte
    /// that was hashed was counted against its file's size, so the two agree.
    fn finish(mut self) -> Vec<PackRef> {
        self.close();
        self.roots
            .into_iter()
            .zip(self.planner.finish())
            .map(|(root, size)| PackRef { root, size })
            .collect()
    }
}

fn no_bytes(path: ManifestPath, kind: Kind, mtime_ms: u64, mode: u32) -> Entry {
    Entry {
        path,
        kind,
        size: 0,
        mtime_ms,
        mode,
        loc: Loc::None,
        link_target: None,
    }
}

fn file(path: ManifestPath, facts: &Facts, loc: Loc) -> Entry {
    Entry {
        path,
        kind: Kind::File,
        size: facts.size,
        mtime_ms: facts.mtime_ms,
        mode: facts.mode,
        loc,
        link_target: None,
    }
}
