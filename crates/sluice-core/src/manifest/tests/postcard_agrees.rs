//! `SPEC.md` section 8 says the bytes are those of postcard 1.x. This reads what the encoder
//! writes with postcard's own deserializer into a mirror of the types in the spec, so the table
//! is checked against postcard and not only against our own reader.

use serde::Deserialize;

use super::common::*;

#[derive(Debug, Deserialize, PartialEq)]
enum MirrorLoc {
    Blob {
        root: [u8; 32],
    },
    Packed {
        pack_index: u32,
        offset: u64,
        len: u64,
    },
    None,
}

#[derive(Debug, Deserialize, PartialEq)]
struct MirrorEntry {
    path: String,
    kind: u8,
    size: u64,
    mtime_ms: u64,
    mode: u32,
    loc: MirrorLoc,
    link_target: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct MirrorPack {
    root: [u8; 32],
    size: u64,
}

#[derive(Debug, Deserialize, PartialEq)]
struct MirrorManifest {
    version: u16,
    entries: Vec<MirrorEntry>,
    packs: Vec<MirrorPack>,
}

#[test]
fn postcard_reads_the_bytes_of_the_encoder_as_the_fields_of_the_spec() {
    let bytes = with_packs(
        &[
            dir("d"),
            Raw {
                size: varint(300),
                mtime: varint(1_800_000_000_000),
                mode: varint(0o4755),
                ..file("d/big")
            },
            symlink("d/l", "big"),
            Raw {
                loc: packed(0, 128, 5),
                ..file("d/small")
            },
        ],
        &[(7, 70_000)],
    );
    let manifest = decode(&bytes).expect("a manifest");
    // What the encoder writes is what the hand-written bytes are, and postcard reads it.
    assert_eq!(manifest.encode(), bytes);
    let read: MirrorManifest = postcard::from_bytes(&bytes).expect("postcard reads it");

    assert_eq!(read.version, 1);
    assert_eq!(read.entries.len(), 4);
    assert_eq!(read.entries[0].kind, 1);
    assert_eq!(read.entries[0].loc, MirrorLoc::None);
    assert_eq!(read.entries[1].path, "d/big");
    assert_eq!(read.entries[1].size, 300);
    assert_eq!(read.entries[1].mtime_ms, 1_800_000_000_000);
    assert_eq!(read.entries[1].mode, 0o4755);
    assert_eq!(read.entries[1].loc, MirrorLoc::Blob { root: [0xab; 32] });
    assert_eq!(
        read.entries[3].loc,
        MirrorLoc::Packed {
            pack_index: 0,
            offset: 128,
            len: 5
        }
    );
    assert_eq!(read.entries[2].kind, 2);
    assert_eq!(read.entries[2].link_target.as_deref(), Some("big"));
    assert_eq!(read.entries[1].link_target, None);
    assert_eq!(
        read.packs,
        [MirrorPack {
            root: [7; 32],
            size: 70_000
        }]
    );
}

#[test]
fn the_mirror_is_what_postcard_writes_for_it() {
    // The other direction: postcard's own bytes for a mirror are bytes the decoder accepts.
    // path, kind, size, mtime_ms, mode, the `Blob` tag with its root, link_target
    type Row<'a> = (&'a str, u8, u64, u64, u32, (u32, [u8; 32]), Option<&'a str>);
    #[derive(serde::Serialize)]
    struct Out<'a> {
        version: u16,
        entries: Vec<Row<'a>>,
        packs: Vec<([u8; 32], u64)>,
    }
    let out = Out {
        version: 1,
        entries: vec![("f", 0, 7, 8, 0o600, (0, [3; 32]), None)],
        packs: vec![],
    };
    let bytes = postcard::to_allocvec(&out).expect("postcard writes it");
    let manifest = decode(&bytes).expect("the decoder reads postcard's bytes");
    assert_eq!(manifest.entries()[0].size, 7);
    assert_eq!(manifest.encode(), bytes);
}
