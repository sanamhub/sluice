//! Property tests. A manifest that is valid goes through `encode` and `decode` byte for byte,
//! bytes that are not a manifest never panic the decoder, and what it accepts has no collision.

use std::collections::BTreeMap;

use proptest::prelude::*;

use super::common::path;
use crate::manifest::rules::collision_key;
use crate::manifest::{Entry, Kind, Limits, Loc, Manifest, PackRef, link::LinkTarget, wire};

/// Names that never collide with each other, so a tree made from them is always valid. There is
/// no `f` for `F` and no `ss` for `ß-x`.
const NAMES: &[&str] = &[
    "a", "b", "c", "d.txt", "日本", "é", "ß-x", "x y", "dir", "F", "Z9",
];
/// Names that do collide, and the case variants and folds of each other.
const COLLIDING: &[&str] = &[
    "a", "A", "ß", "SS", "ss", "Ss", "k", "K", "ǰ", "É", "é", "x",
];
/// Targets that pass link target rules 1 to 5 from any folder, because they never use `..`.
const TARGETS: &[&str] = &["x", "a/b", "c/d/e", "日本"];

#[derive(Debug, Clone)]
struct Attrs {
    pick: u8,
    root: [u8; 32],
    size: u64,
    mtime_ms: u64,
    mode: u32,
    spot: u64,
    target: usize,
}

fn attrs() -> impl Strategy<Value = Attrs> {
    (
        any::<u8>(),
        any::<[u8; 32]>(),
        any::<u64>(),
        any::<u64>(),
        0..=0o7777u32,
        any::<u64>(),
        0..TARGETS.len(),
    )
        .prop_map(|(pick, root, size, mtime_ms, mode, spot, target)| Attrs {
            pick,
            root,
            size,
            mtime_ms,
            mode,
            spot,
            target,
        })
}

fn packs() -> impl Strategy<Value = Vec<PackRef>> {
    prop::collection::vec(
        (any::<[u8; 32]>(), 0..100_000u64).prop_map(|(root, size)| PackRef { root, size }),
        0..4,
    )
}

/// One entry for `path`, of the kind `attrs.pick` selects. `is_parent` forces a folder.
fn entry(path_text: &str, attrs: &Attrs, is_parent: bool, packs: &[PackRef]) -> Entry {
    let kind_pick = if is_parent { 1 } else { attrs.pick % 4 };
    let (kind, size, loc, link_target) = match kind_pick {
        1 => (Kind::Dir, 0, Loc::None, None),
        2 => {
            let target = LinkTarget::parse(TARGETS[attrs.target]).expect("a target");
            (Kind::Symlink, 0, Loc::None, Some(target))
        }
        3 if !packs.is_empty() => {
            let pack_index = (attrs.spot % packs.len() as u64) as u32;
            let room = packs[pack_index as usize].size;
            let len = attrs.size % (room + 1);
            let offset = (attrs.spot / 7) % (room - len + 1);
            let loc = Loc::Packed {
                pack_index,
                offset,
                len,
            };
            (Kind::File, len, loc, None)
        }
        _ => (Kind::File, attrs.size, Loc::Blob { root: attrs.root }, None),
    };
    Entry {
        path: path(path_text),
        kind,
        size,
        mtime_ms: attrs.mtime_ms,
        mode: attrs.mode,
        loc,
        link_target,
    }
}

/// A tree made from `names`: each path of the list, and every folder above it, sorted. It is
/// valid as long as no two names in the pool collide.
fn tree(
    names: &'static [&'static str],
    max_depth: usize,
) -> impl Strategy<Value = (Vec<Entry>, Vec<PackRef>)> {
    let paths = prop::collection::vec(
        (
            prop::collection::vec(0..names.len(), 1..=max_depth),
            attrs(),
        ),
        0..12,
    );
    (paths, packs()).prop_map(move |(paths, packs)| {
        let mut texts: BTreeMap<String, Attrs> = BTreeMap::new();
        let mut parents = std::collections::BTreeSet::new();
        for (parts, attrs) in paths {
            let parts: Vec<&str> = parts.into_iter().map(|i| names[i]).collect();
            for depth in 1..parts.len() {
                parents.insert(parts[..depth].join("/"));
            }
            texts.insert(parts.join("/"), attrs);
        }
        let blank = Attrs {
            pick: 1,
            root: [0; 32],
            size: 0,
            mtime_ms: 0,
            mode: 0o755,
            spot: 0,
            target: 0,
        };
        for parent in &parents {
            texts.entry(parent.clone()).or_insert_with(|| blank.clone());
        }
        let entries = texts
            .iter()
            .map(|(text, attrs)| entry(text, attrs, parents.contains(text), &packs))
            .collect();
        (entries, packs)
    })
}

fn valid() -> impl Strategy<Value = Manifest> {
    tree(NAMES, 3).prop_map(|(entries, packs)| {
        Manifest::new(entries, packs).expect("a tree of names that never collide is valid")
    })
}

proptest! {
    #[test]
    fn a_valid_manifest_goes_through_encode_and_decode_byte_for_byte(manifest in valid()) {
        let bytes = manifest.encode();
        let decoded = Manifest::decode(&bytes, Limits::default()).expect("what new accepts");
        prop_assert_eq!(&decoded, &manifest);
        prop_assert_eq!(decoded.encode(), bytes.clone());
        prop_assert_eq!(manifest.root(), *blake3::hash(&bytes).as_bytes());
    }

    #[test]
    fn random_bytes_never_panic_and_what_decodes_writes_back_to_the_same_bytes(
        bytes in prop::collection::vec(any::<u8>(), 0..200)
    ) {
        if let Ok(manifest) = Manifest::decode(&bytes, Limits::default()) {
            prop_assert_eq!(manifest.encode(), bytes);
        }
    }

    #[test]
    fn a_damaged_manifest_is_refused_or_is_a_manifest_with_one_spelling(
        manifest in valid(),
        at in any::<prop::sample::Index>(),
        value in any::<u8>(),
        how in 0..3u8,
    ) {
        let mut bytes = manifest.encode();
        let at = at.index(bytes.len());
        match how {
            0 => bytes[at] = value,
            1 => bytes.truncate(at),
            _ => bytes.insert(at, value),
        }
        if let Ok(decoded) = Manifest::decode(&bytes, Limits::default()) {
            prop_assert_eq!(decoded.encode(), bytes);
        }
    }

    #[test]
    fn what_the_decoder_accepts_has_no_two_entries_that_collide(
        (entries, packs) in tree(COLLIDING, 2)
    ) {
        // Written without `Manifest::new`, which would refuse a collision before the decoder
        // saw it. The decoder is what this checks.
        let bytes = wire::write(&entries, &packs);
        if let Ok(decoded) = Manifest::decode(&bytes, Limits::default()) {
            let keys: Vec<String> =
                decoded.entries().iter().map(|e| collision_key(e.path.as_str())).collect();
            for (at, key) in keys.iter().enumerate() {
                prop_assert!(!keys[..at].contains(key), "{key:?} appears twice");
            }
        }
    }

    #[test]
    fn a_collision_is_refused_by_the_decoder_and_by_new_alike(
        (entries, packs) in tree(COLLIDING, 2)
    ) {
        let bytes = wire::write(&entries, &packs);
        let decoded = Manifest::decode(&bytes, Limits::default()).is_ok();
        let built = Manifest::new(entries, packs).is_ok();
        prop_assert_eq!(decoded, built);
    }
}
