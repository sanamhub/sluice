//! Packing at the sender, checked against the files on disk: the pack bytes are the files end to
//! end, the root is their hash, and the unpacker gives every file back.

use std::fs::{self, OpenOptions};

use super::common::*;
use crate::Error;
use crate::manifest::build::build;
use crate::manifest::pack::{PACK_THRESHOLD, Unpacker};
use crate::manifest::{Built, Kind, Limits, Loc};

/// The bytes of pack `index`, read back from the files that the manifest puts in it.
fn pack_bytes(built: &Built, index: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (at, entry) in built.manifest.entries().iter().enumerate() {
        if matches!(entry.loc, Loc::Packed { pack_index, .. } if pack_index == index) {
            let disk = built.disk.file(at).expect("a file has a disk path");
            bytes.extend(fs::read(disk).expect("read the file"));
        }
    }
    bytes
}

#[test]
fn a_real_folder_packs_its_small_files_and_unpacks_to_the_same_bytes() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "docs/b.txt", b"second file");
    put(dir.path(), "docs/a.txt", b"first");
    put(dir.path(), "docs/deep/c.bin", &[7; 3000]);
    put(dir.path(), "empty", b"");
    let large: Vec<u8> = (0..PACK_THRESHOLD).map(|n| (n % 251) as u8).collect();
    put(dir.path(), "large.bin", &large);
    put(dir.path(), "z", b"last");

    let built = ready(dir.path());
    let packs = built.manifest.packs();
    assert_eq!(packs.len(), 1);
    let bytes = pack_bytes(&built, 0);
    assert_eq!(packs[0].size, bytes.len() as u64);
    assert_eq!(packs[0].root, *blake3::hash(&bytes).as_bytes());

    let mut unpacker = Unpacker::new(&built.manifest, 0).expect("a pack");
    let mut got: Vec<Vec<u8>> = vec![Vec::new(); built.manifest.entries().len()];
    for piece in bytes.chunks(7) {
        for run in unpacker.feed(piece).expect("within the pack") {
            got[run.entry].extend_from_slice(&piece[run.bytes]);
        }
    }
    assert!(unpacker.finish().expect("the whole pack").is_empty());
    for (at, entry) in built.manifest.entries().iter().enumerate() {
        match entry.loc {
            Loc::Packed { .. } => {
                let disk = built.disk.file(at).expect("a disk path");
                assert_eq!(
                    got[at],
                    fs::read(disk).expect("read"),
                    "{}",
                    entry.path.as_str()
                );
            }
            Loc::Blob { root } => {
                let disk = built.disk.file(at).expect("a disk path");
                assert_eq!(
                    root,
                    *blake3::hash(&fs::read(disk).expect("read")).as_bytes()
                );
            }
            Loc::None => assert_ne!(entry.kind, Kind::File),
        }
    }
}

#[test]
fn files_are_packed_in_manifest_order_whatever_the_folders() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    // In byte order `a-c` comes before `a/z`, because `-` is 0x2D and `/` is 0x2F.
    put(dir.path(), "a/z", b"3");
    put(dir.path(), "a-c", b"22");
    put(dir.path(), "b", b"4444");
    let built = ready(dir.path());
    let offsets: Vec<(&str, u64)> = built
        .manifest
        .entries()
        .iter()
        .filter_map(|e| match e.loc {
            Loc::Packed { offset, .. } => Some((e.path.as_str(), offset)),
            _ => None,
        })
        .collect();
    assert_eq!(offsets, [("a-c", 0), ("a/z", 2), ("b", 3)]);
    assert_eq!(pack_bytes(&built, 0), b"2234444");
}

#[test]
fn a_file_that_grows_while_it_is_read_into_a_pack_fails_the_build() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a.txt", b"one");
    put(dir.path(), "b.txt", b"two");
    let result = build(dir.path(), Limits::default(), |path| {
        if path.ends_with("b.txt") {
            let mut file = OpenOptions::new().append(true).open(path).expect("open");
            std::io::Write::write_all(&mut file, b" more").expect("append");
        }
    });
    assert!(matches!(result, Err(Error::Storage(_))), "{result:?}");
}

#[test]
fn a_file_of_its_own_that_grows_while_it_is_read_fails_the_build() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "large.bin", &vec![1; PACK_THRESHOLD as usize]);
    let result = build(dir.path(), Limits::default(), |path| {
        let mut file = OpenOptions::new().append(true).open(path).expect("open");
        std::io::Write::write_all(&mut file, b"x").expect("append");
    });
    assert!(matches!(result, Err(Error::Storage(_))), "{result:?}");
}

#[test]
fn a_folder_of_empty_files_makes_no_pack() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    put(dir.path(), "a", b"");
    put(dir.path(), "b", b"");
    let built = ready(dir.path());
    assert!(built.manifest.packs().is_empty());
    let empty = Loc::Blob {
        root: *blake3::hash(b"").as_bytes(),
    };
    assert!(built.manifest.entries().iter().all(|e| e.loc == empty));
}
