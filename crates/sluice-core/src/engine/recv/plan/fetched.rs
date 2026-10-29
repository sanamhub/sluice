//! The bytes the content requests will move, which is what the room check and the progress total
//! count: every taken file's bytes, and each pack that holds a taken file whole.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Error,
    manifest::{Kind, Loc, Manifest, PackRef},
};

pub(in crate::engine) fn size_overflow() -> Error {
    Error::Policy("the manifest's file sizes add up past u64::MAX".to_string())
}

/// The indexes of the packs that hold a taken file. A pack is one blob, so taking one file inside it
/// asks for the whole pack.
pub(in crate::engine) fn packs_of(manifest: &Manifest, chosen: &[usize]) -> BTreeSet<u32> {
    let mut packs = BTreeSet::new();
    for &at in chosen {
        if let Loc::Packed { pack_index, .. } = manifest.entries()[at].loc {
            packs.insert(pack_index);
        }
    }
    packs
}

/// The pack at `index`, or the refusal a manifest that names one it does not list earns. The rules
/// already refuse such a manifest, so this is a bug if it is ever reached.
pub(in crate::engine) fn pack(manifest: &Manifest, index: u32) -> Result<&PackRef, Error> {
    manifest.packs().get(index as usize).ok_or_else(|| {
        Error::Policy("the manifest points at a pack that is not listed".to_string())
    })
}

/// How many bytes the content requests will move: every pack that holds a taken file, whole, and
/// every taken file that is a blob of its own, counted once per entry that names it. A selection of
/// one small packed file therefore moves its whole pack. The manifest's pack sizes are the sums of
/// the files laid out in each pack by rule 16, and the size a pack really has is proved before its
/// body is requested, so this is an estimate the sender cannot inflate for free.
pub(in crate::engine) fn fetched_bytes(
    manifest: &Manifest,
    chosen: &[usize],
) -> Result<u64, Error> {
    let mut total = 0u64;
    for index in packs_of(manifest, chosen) {
        total = total
            .checked_add(pack(manifest, index)?.size)
            .ok_or_else(size_overflow)?;
    }
    for bytes in shared_bytes(manifest, chosen)?.into_values() {
        total = total.checked_add(bytes).ok_or_else(size_overflow)?;
    }
    Ok(total)
}

/// How many bytes each blob root holds among the entries taken, counted once per entry that names
/// it. A hostile manifest can make these add up past `u64::MAX`, which would wrap the progress
/// total.
pub(in crate::engine) fn shared_bytes(
    manifest: &Manifest,
    chosen: &[usize],
) -> Result<BTreeMap<[u8; 32], u64>, Error> {
    let mut shares: BTreeMap<[u8; 32], u64> = BTreeMap::new();
    for &at in chosen {
        let entry = &manifest.entries()[at];
        if entry.kind != Kind::File {
            continue;
        }
        if let Loc::Blob { root } = entry.loc {
            let total = shares.entry(root).or_default();
            *total = total.checked_add(entry.size).ok_or_else(size_overflow)?;
        }
    }
    Ok(shares)
}
