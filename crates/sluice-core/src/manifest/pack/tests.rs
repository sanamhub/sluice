//! The packing algorithm at its boundaries, and the unpacker against the bytes it splits.

use proptest::prelude::*;

use super::*;
use crate::Error;
use crate::manifest::path::ManifestPath;
use crate::manifest::{Entry, Kind, Limits, Loc, Manifest, PackRef};

const T: u64 = PACK_THRESHOLD;
const G: u64 = PACK_TARGET;

fn packed(pack_index: u32, offset: u64, len: u64) -> Place {
    Place::Packed {
        pack_index,
        offset,
        len,
    }
}

/// 64 files of `T - 1` bytes, which leave a pack 64 bytes short of `PACK_TARGET`.
fn nearly_full() -> Vec<Option<u64>> {
    vec![Some(T - 1); 64]
}

#[test]
fn a_file_one_byte_under_the_threshold_is_packed_alone_and_one_at_it_is_a_blob() {
    let out = layout(&[Some(T - 1), Some(T)]);
    assert_eq!(out.places, [packed(0, 0, T - 1), Place::Blob]);
    assert_eq!(out.pack_sizes, [T - 1]);
}

#[test]
fn a_pack_that_ends_exactly_at_the_target_is_not_closed_early() {
    let mut sizes = nearly_full();
    sizes.push(Some(64));
    sizes.push(Some(1));
    let out = layout(&sizes);
    assert_eq!(out.places[64], packed(0, G - 64, 64));
    assert_eq!(out.places[65], packed(1, 0, 1));
    assert_eq!(out.pack_sizes, [G, 1]);
}

#[test]
fn a_file_that_would_take_the_pack_one_byte_past_the_target_opens_the_next() {
    let mut sizes = nearly_full();
    sizes.push(Some(65));
    let out = layout(&sizes);
    assert_eq!(out.places[64], packed(1, 0, 65));
    assert_eq!(out.pack_sizes, [G - 64, 65]);
}

#[test]
fn empty_files_are_blobs_and_make_no_pack() {
    let out = layout(&[Some(0), Some(0), Some(0)]);
    assert_eq!(out.places, [Place::Blob; 3]);
    assert!(out.pack_sizes.is_empty());
    assert!(layout(&[]).pack_sizes.is_empty());
}

#[test]
fn folders_links_and_large_files_do_not_close_the_open_pack() {
    let out = layout(&[Some(5), None, Some(T), Some(0), Some(6)]);
    assert_eq!(
        out.places,
        [
            packed(0, 0, 5),
            Place::None,
            Place::Blob,
            Place::Blob,
            packed(0, 5, 6)
        ]
    );
    assert_eq!(out.pack_sizes, [11]);
}

#[test]
fn three_packs_fill_in_order() {
    // 64 files of `T - 1` bytes fit in a pack and a 65th does not, so 129 make three packs.
    let out = layout(&vec![Some(T - 1); 129]);
    assert_eq!(out.pack_sizes, [64 * (T - 1), 64 * (T - 1), T - 1]);
    assert_eq!(out.places[64], packed(1, 0, T - 1));
    assert_eq!(out.places[128], packed(2, 0, T - 1));
}

/// A manifest of files `f00000`, `f00001`, ... with these contents, laid out by [`layout`], and
/// the bytes of each pack.
fn packed_manifest(contents: &[Vec<u8>]) -> (Manifest, Vec<Vec<u8>>) {
    let sizes: Vec<Option<u64>> = contents.iter().map(|c| Some(c.len() as u64)).collect();
    let out = layout(&sizes);
    let mut packs = vec![Vec::new(); out.pack_sizes.len()];
    let entries = contents
        .iter()
        .zip(&out.places)
        .enumerate()
        .map(|(at, (content, place))| {
            let loc = match *place {
                Place::Packed {
                    pack_index,
                    offset,
                    len,
                } => {
                    packs[pack_index as usize].extend_from_slice(content);
                    Loc::Packed {
                        pack_index,
                        offset,
                        len,
                    }
                }
                _ => Loc::Blob {
                    root: *blake3::hash(content).as_bytes(),
                },
            };
            Entry {
                path: ManifestPath::parse(&format!("f{at:05}")).expect("a path"),
                kind: Kind::File,
                size: content.len() as u64,
                mtime_ms: 0,
                mode: 0o644,
                loc,
                link_target: None,
            }
        })
        .collect();
    let refs = packs
        .iter()
        .map(|bytes| PackRef {
            root: *blake3::hash(bytes).as_bytes(),
            size: bytes.len() as u64,
        })
        .collect();
    let manifest = Manifest::new(entries, refs).expect("the layout keeps rule 16");
    (manifest, packs)
}

/// Feeds `pack` in pieces cut at `cuts` and gives back each file's bytes, checking that each file
/// ends exactly once, after its last byte, and that files come in manifest order.
fn unpack(manifest: &Manifest, index: u32, pack: &[u8], cuts: &[usize]) -> Vec<(usize, Vec<u8>)> {
    let mut unpacker = Unpacker::new(manifest, index).expect("a pack");
    let mut files: Vec<(usize, Vec<u8>, bool)> = Vec::new();
    let mut take = |runs: Vec<Run>, piece: &[u8]| {
        for run in runs {
            match files.last_mut() {
                Some(last) if last.0 == run.entry && !last.2 => {
                    last.1.extend_from_slice(&piece[run.bytes.clone()]);
                    last.2 = run.ends;
                }
                last => {
                    assert!(last.is_none_or(|last| last.2 && last.0 < run.entry));
                    files.push((run.entry, piece[run.bytes.clone()].to_vec(), run.ends));
                }
            }
        }
    };
    let mut start = 0;
    for &cut in cuts.iter().chain([pack.len()].iter()) {
        let cut = cut.clamp(start, pack.len());
        let piece = &pack[start..cut];
        take(unpacker.feed(piece).expect("within the pack"), piece);
        start = cut;
    }
    unpacker.finish().expect("the whole pack");
    assert!(
        files.iter().all(|(_, _, ended)| *ended),
        "a file never ended"
    );
    files
        .into_iter()
        .map(|(entry, bytes, _)| (entry, bytes))
        .collect()
}

fn expected(manifest: &Manifest, contents: &[Vec<u8>], index: u32) -> Vec<(usize, Vec<u8>)> {
    manifest
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.loc, Loc::Packed { pack_index, .. } if pack_index == index))
        .map(|(at, _)| (at, contents[at].clone()))
        .collect()
}

#[test]
fn the_unpacker_fed_one_byte_at_a_time_gives_every_file_back() {
    let contents: Vec<Vec<u8>> = ["hello", "", "world", "", "!"]
        .iter()
        .map(|s| s.as_bytes().to_vec())
        .collect();
    let (manifest, packs) = packed_manifest(&contents);
    assert_eq!(packs, [b"helloworld!".to_vec()]);
    let cuts: Vec<usize> = (1..packs[0].len()).collect();
    // The empty files are blobs here, as the sender makes them, so only three files are in it.
    assert_eq!(
        unpack(&manifest, 0, &packs[0], &cuts),
        expected(&manifest, &contents, 0)
    );
}

#[test]
fn a_packed_file_of_no_bytes_is_a_run_of_its_own_wherever_it_sits() {
    let file = |name: &str, offset: u64, len: u64| Entry {
        path: ManifestPath::parse(name).expect("a path"),
        kind: Kind::File,
        size: len,
        mtime_ms: 0,
        mode: 0,
        loc: Loc::Packed {
            pack_index: 0,
            offset,
            len,
        },
        link_target: None,
    };
    let entries = vec![file("a", 0, 0), file("b", 0, 3), file("c", 3, 0)];
    let pack = PackRef {
        root: [0; 32],
        size: 3,
    };
    let manifest = Manifest::new(entries, vec![pack]).expect("rule 16 allows them");
    let mut unpacker = Unpacker::new(&manifest, 0).expect("a pack");
    let runs = unpacker.feed(b"xyz").expect("within the pack");
    let shape: Vec<(usize, usize, bool)> = runs
        .iter()
        .map(|r| (r.entry, r.bytes.len(), r.ends))
        .collect();
    assert_eq!(shape, [(0, 0, true), (1, 3, true), (2, 0, true)]);
    unpacker.finish().expect("complete");
}

#[test]
fn too_many_or_too_few_bytes_are_a_verify_error() {
    let contents = vec![b"abc".to_vec(), b"de".to_vec()];
    let (manifest, packs) = packed_manifest(&contents);
    let mut over = Unpacker::new(&manifest, 0).expect("a pack");
    assert!(matches!(over.feed(b"abcdef"), Err(Error::Verify(_))));
    let mut late = Unpacker::new(&manifest, 0).expect("a pack");
    late.feed(&packs[0]).expect("the whole pack");
    assert!(matches!(late.feed(b"x"), Err(Error::Verify(_))));
    let mut short = Unpacker::new(&manifest, 0).expect("a pack");
    short.feed(&packs[0][..4]).expect("within the pack");
    assert!(matches!(short.finish(), Err(Error::Verify(_))));
    assert!(matches!(Unpacker::new(&manifest, 1), Err(Error::Policy(_))));
}

fn sizes() -> impl Strategy<Value = Vec<Option<u64>>> {
    let size = prop_oneof![
        Just(None),
        (0..4u64).prop_map(Some),
        (T - 3..T + 3).prop_map(Some),
        (1..T).prop_map(Some),
        (G - 3..G + 3).prop_map(Some),
    ];
    prop::collection::vec(size, 0..200)
}

proptest! {
    #[test]
    fn any_list_of_sizes_gives_a_layout_the_decoder_accepts(sizes in sizes()) {
        let out = layout(&sizes);
        prop_assert_eq!(out.places.len(), sizes.len());
        prop_assert!(out.pack_sizes.iter().all(|&size| (1..=G).contains(&size)));
        let places = sizes.iter().zip(&out.places).enumerate();
        let entries: Vec<Entry> = places.map(|(at, (size, place))| {
            let (kind, size, loc) = match (size, *place) {
                (None, _) => (Kind::Dir, 0, Loc::None),
                (Some(size), Place::Packed { pack_index, offset, len }) => {
                    (Kind::File, *size, Loc::Packed { pack_index, offset, len })
                }
                (Some(size), _) => (Kind::File, *size, Loc::Blob { root: [1; 32] }),
            };
            Entry {
                path: ManifestPath::parse(&format!("e{at:05}")).expect("a path"),
                kind,
                size,
                mtime_ms: 0,
                mode: 0,
                loc,
                link_target: None,
            }
        }).collect();
        let packs = out.pack_sizes.iter().map(|&size| PackRef { root: [2; 32], size }).collect();
        let manifest = Manifest::new(entries, packs).expect("the layout keeps every rule");
        let decoded = Manifest::decode(&manifest.encode(), Limits::default());
        prop_assert_eq!(decoded.ok(), Some(manifest));
    }

    #[test]
    fn unpacking_with_any_split_gives_every_file_back_in_order(
        contents in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 0..30),
        cuts in prop::collection::vec(0..1200usize, 0..20),
    ) {
        let (manifest, packs) = packed_manifest(&contents);
        let mut cuts = cuts;
        cuts.sort_unstable();
        for (index, pack) in packs.iter().enumerate() {
            let index = index as u32;
            let want = expected(&manifest, &contents, index);
            prop_assert_eq!(unpack(&manifest, index, pack, &cuts), want);
        }
    }
}
