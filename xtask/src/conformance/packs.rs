//! The `packs` vectors. A case is the entries of a manifest with their sizes, and the loc of each
//! and the size of each pack that the packing algorithm gives. A case with content also gives the
//! roots of the blobs, the packs and the manifest, and the runner unpacks each pack and compares
//! every file.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::manifest::pack::{Place, Unpacker, layout};
use sluice_core::manifest::path::ManifestPath;
use sluice_core::manifest::{Entry, Kind, Loc, Manifest, PackRef};

use super::manifests::{hex_array, hex_bytes, number};
use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    entries: Vec<EntryFields>,
    packs: Vec<PackFields>,
    /// Cases with content. The BLAKE3 hash of the manifest's bytes, as hex.
    manifest_root: Option<String>,
}

#[derive(Deserialize)]
struct EntryFields {
    path: String,
    kind: String,
    /// Files. A decimal string.
    size: Option<String>,
    /// Cases with content. The file's bytes as hex.
    content: Option<String>,
    mtime_ms: Option<String>,
    mode: Option<u32>,
    loc: LocFields,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LocFields {
    Blob {
        root: Option<String>,
    },
    Packed {
        pack_index: u32,
        offset: String,
        len: String,
    },
    None,
}

#[derive(Deserialize)]
struct PackFields {
    size: String,
    root: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("packs", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let what = format!("{:?}", case.name);
    check(case).err().map(|reason| format!("{what}: {reason}"))
}

fn check(case: &Case) -> Result<(), String> {
    let sizes = case
        .entries
        .iter()
        .map(|e| match e.kind.as_str() {
            "file" => number(e.size.as_deref().unwrap_or("")).map(Some),
            "dir" | "symlink" => Ok(None),
            other => Err(format!("kind {other}")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let out = layout(&sizes);
    for ((entry, place), at) in case.entries.iter().zip(&out.places).zip(0..) {
        if !same_place(&entry.loc, *place)? {
            return Err(format!(
                "entry {at} is placed {place:?}, not as the case says"
            ));
        }
    }
    let pack_sizes = case
        .packs
        .iter()
        .map(|p| number(&p.size))
        .collect::<Result<Vec<_>, _>>()?;
    if out.pack_sizes != pack_sizes {
        return Err(format!(
            "pack sizes {:?}, not {pack_sizes:?}",
            out.pack_sizes
        ));
    }
    match &case.manifest_root {
        Some(root) => check_content(case, root),
        None => Ok(()),
    }
}

fn same_place(expected: &LocFields, got: Place) -> Result<bool, String> {
    Ok(match (expected, got) {
        (LocFields::Blob { .. }, Place::Blob) | (LocFields::None, Place::None) => true,
        (
            LocFields::Packed {
                pack_index,
                offset,
                len,
            },
            Place::Packed {
                pack_index: index,
                offset: at,
                len: got_len,
            },
        ) => *pack_index == index && number(offset)? == at && number(len)? == got_len,
        _ => false,
    })
}

/// The contents give every root: of a blob, of a pack as its files end to end, and of the
/// manifest. Each pack is then split back by the unpacker and compared file by file.
fn check_content(case: &Case, manifest_root: &str) -> Result<(), String> {
    let contents = case
        .entries
        .iter()
        .map(|e| hex_bytes(e.content.as_deref().unwrap_or("")))
        .collect::<Result<Vec<_>, _>>()?;
    let entries = case
        .entries
        .iter()
        .zip(&contents)
        .map(|(e, content)| entry(e, content))
        .collect::<Result<Vec<_>, _>>()?;
    let mut pack_bytes = vec![Vec::new(); case.packs.len()];
    for (entry, content) in entries.iter().zip(&contents) {
        if let Loc::Packed { pack_index, .. } = entry.loc {
            let pack = pack_bytes
                .get_mut(pack_index as usize)
                .ok_or("no such pack")?;
            pack.extend_from_slice(content);
        }
    }
    let packs = case
        .packs
        .iter()
        .zip(&pack_bytes)
        .map(|(fields, bytes)| {
            let root = hex_array(fields.root.as_deref().ok_or("a pack with no root")?)?;
            if root != *blake3::hash(bytes).as_bytes() {
                return Err("a pack root is not the hash of its files".to_string());
            }
            Ok(PackRef {
                root,
                size: bytes.len() as u64,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let manifest = Manifest::new(entries, packs).map_err(|e| e.to_string())?;
    if manifest.root() != hex_array(manifest_root)? {
        return Err("the manifest root is not the one in `manifest_root`".to_string());
    }
    for (index, bytes) in pack_bytes.iter().enumerate() {
        unpacked_matches(&manifest, index as u32, bytes, &contents)?;
    }
    Ok(())
}

fn entry(fields: &EntryFields, content: &[u8]) -> Result<Entry, String> {
    let (kind, size) = match fields.kind.as_str() {
        "file" => (Kind::File, number(fields.size.as_deref().unwrap_or(""))?),
        "dir" => (Kind::Dir, 0),
        other => return Err(format!("kind {other} in a case with content")),
    };
    if kind == Kind::File && size != content.len() as u64 {
        return Err("a content that is not `size` bytes".to_string());
    }
    let loc = match &fields.loc {
        LocFields::Blob { root } => {
            let root = hex_array(root.as_deref().ok_or("a blob with no root")?)?;
            if root != *blake3::hash(content).as_bytes() {
                return Err("a blob root is not the hash of its content".to_string());
            }
            Loc::Blob { root }
        }
        LocFields::Packed {
            pack_index,
            offset,
            len,
        } => Loc::Packed {
            pack_index: *pack_index,
            offset: number(offset)?,
            len: number(len)?,
        },
        LocFields::None => Loc::None,
    };
    Ok(Entry {
        path: ManifestPath::parse(&fields.path).map_err(|e| e.to_string())?,
        kind,
        size,
        mtime_ms: number(fields.mtime_ms.as_deref().unwrap_or(""))?,
        mode: fields.mode.ok_or("an entry with no mode")?,
        loc,
        link_target: None,
    })
}

/// Feeds the pack in pieces of 3 bytes and checks that each file gets its content back.
fn unpacked_matches(
    manifest: &Manifest,
    index: u32,
    bytes: &[u8],
    contents: &[Vec<u8>],
) -> Result<(), String> {
    let mut unpacker = Unpacker::new(manifest, index).map_err(|e| e.to_string())?;
    let mut got = vec![Vec::new(); contents.len()];
    for piece in bytes.chunks(3) {
        for run in unpacker.feed(piece).map_err(|e| e.to_string())? {
            got[run.entry].extend_from_slice(&piece[run.bytes]);
        }
    }
    unpacker.finish().map_err(|e| e.to_string())?;
    for (at, entry) in manifest.entries().iter().enumerate() {
        let in_pack = matches!(entry.loc, Loc::Packed { pack_index, .. } if pack_index == index);
        if in_pack && got[at] != contents[at] {
            return Err(format!(
                "entry {at} unpacks to other bytes than its content"
            ));
        }
    }
    Ok(())
}
