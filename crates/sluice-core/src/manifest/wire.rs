//! The manifest's bytes, field by field, as the table in `SPEC.md` section 8 gives them.
//!
//! Writing goes through `postcard`, one field at a time, so the table is postcard's encoding by
//! construction. Reading does not, because the decoder has to name the rule a byte string breaks
//! and has to refuse a varint with a zero byte too many, which postcard accepts. It is a small
//! cursor that reads what the table says and nothing else.

use serde::Serialize;

use super::Limits;
use super::entry::{Entry, Kind, Loc, PackRef};
use super::link::LinkTarget;
use super::path::ManifestPath;
use super::rules::{Checker, Fault, check_packs, rule};

/// The only `version` this release reads and writes.
const VERSION: u16 = 1;
/// The fewest bytes an entry takes: a one-character path with its length, the kind, the size,
/// the time and the mode as one byte each, a `None` loc and an absent link target.
const SHORTEST_ENTRY: usize = 8;
/// A pack takes its 32-byte root and a one-byte size.
const SHORTEST_PACK: usize = 33;
/// How many entries the reader reserves room for before it has seen them. The count comes from a
/// stranger, so the vector grows as entries arrive and is not sized by the count.
const RESERVED_ENTRIES: u64 = 1 << 16;

/// The bytes of a manifest, always in their shortest form.
pub(super) fn write(entries: &[Entry], packs: &[PackRef]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 * entries.len() + 16);
    put(&mut out, &VERSION);
    put(&mut out, &(entries.len() as u64));
    for entry in entries {
        put(&mut out, entry.path.as_str());
        put(&mut out, &(entry.kind as u8));
        put(&mut out, &entry.size);
        put(&mut out, &entry.mtime_ms);
        put(&mut out, &entry.mode);
        put(&mut out, &entry.loc);
        put(
            &mut out,
            &entry.link_target.as_ref().map(LinkTarget::as_str),
        );
    }
    put(&mut out, &(packs.len() as u64));
    for pack in packs {
        put(&mut out, &pack.root);
        put(&mut out, &pack.size);
    }
    out
}

fn put<T: Serialize + ?Sized>(out: &mut Vec<u8>, value: &T) {
    // Postcard fails into a `Vec` only for a type whose `Serialize` returns an error, or for a
    // sequence of unknown length. These are integers, byte arrays, strings and one enum, all of
    // known size, so a failure here is a bug. Skipping the field would write a corrupt manifest
    // with no sign of it, so this stops instead.
    *out = postcard::to_extend(value, std::mem::take(out))
        .expect("postcard encodes a manifest field into a Vec");
}

/// The entries and packs of `bytes`, every rule applied, or the first rule it breaks.
pub(super) fn read(bytes: &[u8], limits: Limits) -> Result<(Vec<Entry>, Vec<PackRef>), Fault> {
    // Before a byte is read, so a manifest that is too big costs one comparison.
    limits.check(bytes.len() as u64)?;
    let mut reader = Reader { rest: bytes };
    // The layout can change with the version, so nothing else is read before it is known.
    if reader.varint(16)? != u64::from(VERSION) {
        return Err(Fault::new(rule::VERSION));
    }
    let count = reader.varint(64)?;
    if count > limits.max_entries {
        return Err(Fault::new(rule::ENTRIES));
    }
    // A count that the bytes left cannot hold is a manifest that ends early. Saying so here
    // spares the loop below from running to the end of the bytes for a count of billions.
    if count > (reader.rest.len() / SHORTEST_ENTRY) as u64 {
        return Err(Fault::new(rule::CUT));
    }
    let mut entries = Vec::with_capacity(count.min(RESERVED_ENTRIES) as usize);
    let mut checker = Checker::default();
    for _ in 0..count {
        let entry = reader.entry()?;
        checker.accept(&entries, &entry)?;
        entries.push(entry);
    }
    let packs = reader.packs()?;
    if !reader.rest.is_empty() {
        return Err(Fault::new(rule::CUT));
    }
    check_packs(&entries, &packs)?;
    Ok((entries, packs))
}

struct Reader<'a> {
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Fault> {
        if n > self.rest.len() {
            return Err(Fault::new(rule::CUT));
        }
        let (head, rest) = self.rest.split_at(n);
        self.rest = rest;
        Ok(head)
    }

    fn byte(&mut self) -> Result<u8, Fault> {
        Ok(self.bytes(1)?[0])
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Fault> {
        let mut out = [0; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    /// A LEB128 varint for an integer of `bits` bits: the shortest form, and no bit above the
    /// width of the integer. A longer form is a second spelling of one manifest.
    fn varint(&mut self, bits: u32) -> Result<u64, Fault> {
        let longest = bits.div_ceil(7);
        let mut value = 0u64;
        for at in 0..longest {
            let byte = self.byte()?;
            let group = u64::from(byte & 0x7f);
            let shift = 7 * at;
            // The last group can hold fewer than 7 bits of the integer.
            let room = bits - shift;
            if room < 7 && group >> room != 0 {
                return Err(Fault::new(rule::VARINT));
            }
            value |= group << shift;
            if byte & 0x80 == 0 {
                // A final group of zero after a longer one is a zero byte too many.
                return if at > 0 && group == 0 {
                    Err(Fault::new(rule::VARINT))
                } else {
                    Ok(value)
                };
            }
        }
        Err(Fault::new(rule::VARINT))
    }

    fn text(&mut self) -> Result<&'a str, Fault> {
        let len = self.varint(64)?;
        let len = usize::try_from(len).map_err(|_| Fault::new(rule::CUT))?;
        std::str::from_utf8(self.bytes(len)?).map_err(|_| Fault::new(rule::TAG))
    }

    fn entry(&mut self) -> Result<Entry, Fault> {
        let path =
            ManifestPath::parse_rule(self.text()?).map_err(|sub| Fault::within(rule::PATH, sub))?;
        let kind = match self.byte()? {
            0 => Kind::File,
            1 => Kind::Dir,
            2 => Kind::Symlink,
            _ => return Err(Fault::new(rule::TAG)),
        };
        let size = self.varint(64)?;
        let mtime_ms = self.varint(64)?;
        let mode = self.varint(32)? as u32;
        let loc = self.loc()?;
        let link_target = self.link_target()?;
        Ok(Entry {
            path,
            kind,
            size,
            mtime_ms,
            mode,
            loc,
            link_target,
        })
    }

    fn loc(&mut self) -> Result<Loc, Fault> {
        match self.varint(32)? {
            0 => Ok(Loc::Blob {
                root: self.array()?,
            }),
            1 => Ok(Loc::Packed {
                pack_index: self.varint(32)? as u32,
                offset: self.varint(64)?,
                len: self.varint(64)?,
            }),
            2 => Ok(Loc::None),
            _ => Err(Fault::new(rule::TAG)),
        }
    }

    fn link_target(&mut self) -> Result<Option<LinkTarget>, Fault> {
        match self.byte()? {
            0 => Ok(None),
            1 => LinkTarget::parse_rule(self.text()?)
                .map(Some)
                .map_err(|sub| Fault::within(rule::LINK_TARGET, sub)),
            _ => Err(Fault::new(rule::TAG)),
        }
    }

    fn packs(&mut self) -> Result<Vec<PackRef>, Fault> {
        let count = self.varint(64)?;
        if count > (self.rest.len() / SHORTEST_PACK) as u64 {
            return Err(Fault::new(rule::CUT));
        }
        (0..count)
            .map(|_| {
                Ok(PackRef {
                    root: self.array()?,
                    size: self.varint(64)?,
                })
            })
            .collect()
    }
}
