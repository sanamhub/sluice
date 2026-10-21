//! The `manifests` vectors. A valid case gives the manifest bytes, the fields they decode to and
//! the root. An invalid case only has to fail. A case can carry its own limits, so that the two
//! limits are tested without a 256 MiB vector.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::manifest::link::LinkTarget;
use sluice_core::manifest::path::ManifestPath;
use sluice_core::manifest::{Entry, Kind, Limits, Loc, Manifest, PackRef};

use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    /// What the case is, for the line that reports it. Its text is not compared.
    name: String,
    /// The manifest as lower-case hex. It can be empty.
    bytes: String,
    valid: bool,
    /// Valid cases. The BLAKE3 hash of `bytes`, as hex.
    root: Option<String>,
    /// Valid cases. What `bytes` decode to.
    fields: Option<Fields>,
    /// Replaces the default limits for this case. Both are decimal strings.
    limits: Option<LimitFields>,
}

#[derive(Deserialize)]
struct LimitFields {
    max_bytes: String,
    max_entries: String,
}

#[derive(Deserialize)]
struct Fields {
    version: u16,
    entries: Vec<EntryFields>,
    packs: Vec<PackFields>,
}

#[derive(Deserialize)]
struct EntryFields {
    path: String,
    kind: String,
    size: String,
    mtime_ms: String,
    mode: u32,
    loc: LocFields,
    link_target: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum LocFields {
    Blob {
        root: String,
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
    root: String,
    size: String,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("manifests", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let what = format!("{:?}", case.name);
    let bytes = match hex_bytes(&case.bytes) {
        Ok(bytes) => bytes,
        Err(reason) => return Some(format!("{what}: `bytes` is not hex: {reason}")),
    };
    let limits = match limits(case.limits.as_ref()) {
        Ok(limits) => limits,
        Err(reason) => return Some(format!("{what}: {reason}")),
    };
    let decoded = Manifest::decode(&bytes, limits);
    if !case.valid {
        return decoded
            .is_ok()
            .then(|| format!("{what}: expected rejected, got accepted"));
    }
    let (Some(root), Some(fields)) = (&case.root, &case.fields) else {
        return Some(format!("{what}: a valid case needs `root` and `fields`"));
    };
    let expected = match expected(fields) {
        Ok(manifest) => manifest,
        Err(reason) => return Some(format!("{what}: the fields are not a manifest: {reason}")),
    };
    match decoded {
        Err(err) => Some(format!("{what}: expected accepted, got {err}")),
        Ok(manifest) if manifest != expected => Some(format!("{what}: decoded to other fields")),
        Ok(manifest) if manifest.encode() != bytes => {
            Some(format!("{what}: encodes to other bytes than `bytes`"))
        }
        Ok(manifest) if !hex_bytes(root).is_ok_and(|r| r == manifest.root()) => {
            Some(format!("{what}: the root is not the one in `root`"))
        }
        Ok(_) => None,
    }
}

fn limits(fields: Option<&LimitFields>) -> Result<Limits, String> {
    let Some(fields) = fields else {
        return Ok(Limits::default());
    };
    Ok(Limits {
        max_bytes: number(&fields.max_bytes)?,
        max_entries: number(&fields.max_entries)?,
    })
}

/// The manifest the fields describe, built through `Manifest::new` and not the decoder, so the
/// decoder is compared with something it did not make.
fn expected(fields: &Fields) -> Result<Manifest, String> {
    if fields.version != 1 {
        return Err(format!("version {}", fields.version));
    }
    let entries = fields
        .entries
        .iter()
        .map(entry)
        .collect::<Result<Vec<_>, _>>()?;
    let packs = fields
        .packs
        .iter()
        .map(|pack| {
            Ok(PackRef {
                root: hex_array(&pack.root)?,
                size: number(&pack.size)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Manifest::new(entries, packs).map_err(|e| e.to_string())
}

fn entry(fields: &EntryFields) -> Result<Entry, String> {
    let kind = match fields.kind.as_str() {
        "file" => Kind::File,
        "dir" => Kind::Dir,
        "symlink" => Kind::Symlink,
        other => return Err(format!("kind {other}")),
    };
    let loc = match &fields.loc {
        LocFields::Blob { root } => Loc::Blob {
            root: hex_array(root)?,
        },
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
        size: number(&fields.size)?,
        mtime_ms: number(&fields.mtime_ms)?,
        mode: fields.mode,
        loc,
        link_target: fields
            .link_target
            .as_deref()
            .map(LinkTarget::parse)
            .transpose()
            .map_err(|e| e.to_string())?,
    })
}

pub(super) fn number(text: &str) -> Result<u64, String> {
    text.parse().map_err(|e| format!("number {text}: {e}"))
}

pub(super) fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        return Err("an odd length or a character that is not hex".to_string());
    }
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
            u8::from_str_radix(pair, 16).map_err(|e| format!("{pair}: {e}"))
        })
        .collect()
}

pub(super) fn hex_array(text: &str) -> Result<[u8; 32], String> {
    hex_bytes(text)?
        .try_into()
        .map_err(|_| format!("{text} is not 32 bytes of hex"))
}
