//! The decoding rules of `SPEC.md` section 8 that a manifest breaks as a whole: the number of
//! each rule, the error that names it, and the checks that compare one entry with the ones before.
//!
//! [`Manifest::decode`](super::Manifest::decode) and [`Manifest::new`](super::Manifest::new) run
//! the same checks from here, so a manifest that one accepts is a manifest the other accepts.

use std::collections::HashSet;

use unicase::UniCase;
use unicode_normalization::UnicodeNormalization;

use super::entry::{Entry, Kind, Loc, PackRef};
use crate::Error;

/// The numbers of the decoding rules in `SPEC.md` section 8.
pub(super) mod rule {
    pub const BYTES: u8 = 1;
    pub const ENTRIES: u8 = 2;
    pub const VERSION: u8 = 3;
    pub const VARINT: u8 = 4;
    pub const TAG: u8 = 5;
    pub const CUT: u8 = 6;
    pub const ORDER: u8 = 7;
    pub const PARENT: u8 = 8;
    pub const PATH: u8 = 9;
    pub const LINK_PRESENCE: u8 = 10;
    pub const LINK_TARGET: u8 = 11;
    pub const COLLISION: u8 = 12;
    pub const KIND_LOC: u8 = 13;
    pub const PACKED: u8 = 14;
    pub const MODE: u8 = 15;
    pub const LAYOUT: u8 = 16;
}

/// A rule a manifest breaks. `sub` is the number of the path rule, link target rule or pack layout
/// rule inside rules 9, 11 and 16. It is a number and never text of the manifest, so the error is
/// safe to log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Fault {
    rule: u8,
    sub: Option<u8>,
}

impl Fault {
    pub(super) const fn new(rule: u8) -> Self {
        Self { rule, sub: None }
    }

    pub(super) const fn within(rule: u8, sub: u8) -> Self {
        Self {
            rule,
            sub: Some(sub),
        }
    }

    pub(super) fn into_error(self) -> Error {
        let which = match (self.rule, self.sub) {
            (rule::PATH, Some(sub)) => format!(" (path rule {sub})"),
            (rule::LINK_TARGET, Some(sub)) => format!(" (link target rule {sub})"),
            (rule::LAYOUT, Some(sub)) => format!(" (pack layout rule {sub})"),
            _ => String::new(),
        };
        Error::Policy(format!(
            "manifest rule {}{which}: the manifest {}",
            self.rule,
            describe(self.rule)
        ))
    }
}

fn describe(rule: u8) -> &'static str {
    match rule {
        rule::BYTES => "is longer than the byte limit",
        rule::ENTRIES => "has more entries than the entry limit",
        rule::VERSION => "has a version this release does not read",
        rule::VARINT => "has a varint that is not in its shortest form or does not fit",
        rule::TAG => "has a kind or tag that is not defined, or text that is not UTF-8",
        rule::CUT => "ends before its last field, or has bytes after it",
        rule::ORDER => "has entries that are not in strictly ascending order of path",
        rule::PARENT => "has an entry whose parent is not a folder entry before it",
        rule::PATH => "has an entry whose path breaks a path rule",
        rule::LINK_PRESENCE => {
            "has a link_target on an entry that is not a symlink, or none on one"
        }
        rule::LINK_TARGET => "has an entry whose link_target breaks a link target rule",
        rule::COLLISION => "has two paths that are the same after case folding",
        rule::KIND_LOC => "has an entry whose kind, loc and size disagree",
        rule::PACKED => "has a packed entry that does not fit its pack",
        rule::MODE => "has a mode above 0o7777",
        _ => "has packs whose files do not lie end to end and fill them",
    }
}

/// The key two paths share when they collide: full Unicode case folding, then NFC. Folding can
/// produce text that is not in NFC (U+0130 gives `i` and a combining dot), and two names whose
/// folds differ only in how they are composed would be one name on a filesystem that normalizes.
pub(super) fn collision_key(path: &str) -> String {
    UniCase::unicode(path).to_folded_case().nfc().collect()
}

/// The checks that compare an entry with the ones before it. It holds a hash of the folded path of
/// every entry accepted so far, so each new entry costs one lookup and the whole list is linear.
///
/// The set holds the first 16 bytes of the BLAKE3 hash of each key, and not the key, because the
/// keys themselves cost about 100 MiB more per million entries (measured on a decode of 1,000,000
/// realistic paths). Two equal keys always have equal hashes, so no collision is missed. A false
/// alarm needs two different keys with the same 128 bits, about one chance in 2^89 at the default
/// limit of 1,000,000 entries, and it would only reject the sender's own manifest, which no
/// attacker gains from.
#[derive(Default)]
pub(super) struct Checker {
    folded: HashSet<[u8; 16]>,
}

impl Checker {
    /// Accepts `next` as the entry after `before`, or names the rule it breaks.
    pub(super) fn accept(&mut self, before: &[Entry], next: &Entry) -> Result<(), Fault> {
        // Strictly ascending, so a repeated path is a rejection and not a tie.
        if before
            .last()
            .is_some_and(|last| last.path.as_str() >= next.path.as_str())
        {
            return Err(Fault::new(rule::ORDER));
        }
        parent_is_a_folder(before, next)?;
        next.check()?;
        let key = short_hash(&collision_key(next.path.as_str()));
        if !self.folded.insert(key) {
            return Err(Fault::new(rule::COLLISION));
        }
        Ok(())
    }
}

fn short_hash(key: &str) -> [u8; 16] {
    let mut short = [0; 16];
    short.copy_from_slice(&blake3::hash(key.as_bytes()).as_bytes()[..16]);
    short
}

/// Rule 8. The entries so far are sorted, and a parent sorts before its children, so a binary
/// search finds it without a second copy of the paths. Only the parent is looked at: its own
/// parent was checked when it was accepted.
fn parent_is_a_folder(before: &[Entry], next: &Entry) -> Result<(), Fault> {
    let Some((parent, _)) = next.path.as_str().rsplit_once('/') else {
        return Ok(());
    };
    match before.binary_search_by(|e| e.path.as_str().cmp(parent)) {
        Ok(at) if before[at].kind == Kind::Dir => Ok(()),
        _ => Err(Fault::new(rule::PARENT)),
    }
}

/// Rules 14 and 16. They need the whole pack list, which comes after the entries in the bytes, so
/// they run once both are known.
pub(super) fn check_packs(entries: &[Entry], packs: &[PackRef]) -> Result<(), Fault> {
    each_packed_entry_fits(entries, packs)?;
    packs_are_laid_out_end_to_end(entries, packs)
}

/// Rule 14, for every packed entry on its own.
fn each_packed_entry_fits(entries: &[Entry], packs: &[PackRef]) -> Result<(), Fault> {
    for entry in entries {
        let Loc::Packed {
            pack_index,
            offset,
            len,
        } = entry.loc
        else {
            continue;
        };
        let fits = usize::try_from(pack_index)
            .ok()
            .and_then(|at| packs.get(at))
            .is_some_and(|pack| {
                len == entry.size && offset.checked_add(len).is_some_and(|end| end <= pack.size)
            });
        if !fits {
            return Err(Fault::new(rule::PACKED));
        }
    }
    Ok(())
}

/// Rule 16: the packed entries, in manifest order, fill the packs one after another with no gap
/// and no overlap, and no pack is left over. Every byte of a pack then has exactly one file, so a
/// receiver knows where each byte goes before it fetches any.
fn packs_are_laid_out_end_to_end(entries: &[Entry], packs: &[PackRef]) -> Result<(), Fault> {
    let fault = |sub| Fault::within(rule::LAYOUT, sub);
    // Rule 14 ran first, so every index names a pack and no `offset + len` wraps.
    let pack_size = |index: u32| {
        usize::try_from(index)
            .ok()
            .and_then(|at| packs.get(at))
            .map(|pack| pack.size)
    };
    let mut open: Option<(u32, u64)> = None;
    for entry in entries {
        let Loc::Packed {
            pack_index,
            offset,
            len,
        } = entry.loc
        else {
            continue;
        };
        let start = match open {
            None if pack_index == 0 => 0,
            Some((index, end)) if pack_index == index => end,
            Some((index, end)) if index.checked_add(1) == Some(pack_index) => {
                if pack_size(index) != Some(end) {
                    return Err(fault(3));
                }
                0
            }
            _ => return Err(fault(1)),
        };
        if offset != start {
            return Err(fault(2));
        }
        open = Some((pack_index, offset + len));
    }
    let Some((last, end)) = open else {
        return if packs.is_empty() {
            Ok(())
        } else {
            Err(fault(4))
        };
    };
    if pack_size(last) != Some(end) {
        return Err(fault(3));
    }
    if packs.len() as u64 != u64::from(last) + 1 {
        return Err(fault(4));
    }
    Ok(())
}

/// Every check that is not about bytes, in one pass over a finished list. This is what
/// [`Manifest::new`](super::Manifest::new) runs.
pub(super) fn check_all(entries: &[Entry], packs: &[PackRef]) -> Result<(), Fault> {
    let mut checker = Checker::default();
    for (at, entry) in entries.iter().enumerate() {
        checker.accept(&entries[..at], entry)?;
    }
    check_packs(entries, packs)
}
