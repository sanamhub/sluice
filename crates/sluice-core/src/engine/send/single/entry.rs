//! The one entry a single-file send carries: the checks its name and stamp pass, the manifest that
//! names it, the metadata kept beside that manifest, and the roots the provider answers for.

use std::{fs, path::Path};

use iroh_blobs::{
    BlobFormat, Hash,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};

use super::super::{Stamp, storage};
use crate::{
    Error,
    manifest::{Entry, Kind, Loc, Manifest, PackRef, path::ManifestPath},
};

/// The name, path and metadata of the one file a single-file send carries.
pub(in crate::engine) fn checked_file(
    path: &Path,
) -> Result<(String, String, ManifestPath, Stamp), Error> {
    let shown = path.display().to_string();
    let meta = fs::symlink_metadata(path).map_err(|e| storage::policy_path("read", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{shown} is a symlink; send the file it points at by its real path"
        )));
    }
    if !meta.is_file() {
        return Err(Error::Policy(format!(
            "{shown} is not a regular file; send it as one path of a transfer with send"
        )));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Policy(format!("{shown} has no UTF-8 file name")))?;
    let entry_path = ManifestPath::parse(name)?;
    Ok((shown, name.to_string(), entry_path, Stamp::of(path)?))
}

pub(super) async fn import(path: &Path, shown: &str, store: &FsStore) -> Result<TempTag, Error> {
    store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.to_path_buf(),
            format: BlobFormat::Raw,
            mode: ImportMode::TryReference,
        })
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot import {shown}: {}", crate::plain(&e))))
}

/// A file of 1 byte to under `PACK_THRESHOLD` is a pack of its own, so the receiver reads it the
/// same way it reads a file inside a folder. Any other file is a blob with its BLAKE3 root.
pub(in crate::engine) fn one_entry_manifest(
    entry_path: ManifestPath,
    before: &Stamp,
    hash: Hash,
) -> Result<Manifest, Error> {
    let (packs, loc) = if before.size > 0 && before.size < crate::manifest::pack::PACK_THRESHOLD {
        (
            vec![PackRef {
                root: *hash.as_bytes(),
                size: before.size,
            }],
            Loc::Packed {
                pack_index: 0,
                offset: 0,
                len: before.size,
            },
        )
    } else {
        (
            vec![],
            Loc::Blob {
                root: *hash.as_bytes(),
            },
        )
    };
    Manifest::new(
        vec![Entry {
            path: entry_path,
            kind: Kind::File,
            size: before.size,
            mtime_ms: before.mtime_ms,
            mode: before.mode,
            loc,
            link_target: None,
        }],
        packs,
    )
}

/// Keeps the manifest and the hash sequence, and checks the store gave each the root its bytes
/// hash to, which is what the ticket and the receiver will name.
pub(super) async fn keep_metadata(
    store: &FsStore,
    manifest: &Manifest,
) -> Result<Vec<TempTag>, Error> {
    let manifest_tag = store
        .blobs()
        .add_bytes(manifest.encode())
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot keep the manifest: {}", crate::plain(&e))))?;
    if *manifest_tag.hash().as_bytes() != manifest.root() {
        return Err(Error::Bug(
            "the store gave the manifest a root other than its BLAKE3 hash".to_string(),
        ));
    }
    let sequence_tag = store
        .blobs()
        .add_bytes(manifest.sequence_bytes())
        .temp_tag()
        .await
        .map_err(|e| {
            Error::Storage(format!(
                "cannot keep the hash sequence: {}",
                crate::plain(&e)
            ))
        })?;
    if *sequence_tag.hash().as_bytes() != manifest.sequence_root() {
        return Err(Error::Bug(
            "the store gave the hash sequence a root other than its BLAKE3 hash".to_string(),
        ));
    }
    // Temporary tags: the collector may free both as soon as this send stops. A named tag would
    // keep them for ever, and every send would add another.
    Ok(vec![manifest_tag, sequence_tag])
}

/// The roots the provider answers for: every content blob and pack, and the sequence itself.
pub(super) fn allowed_roots(manifest: &Manifest) -> std::collections::BTreeSet<[u8; 32]> {
    let mut allowed: std::collections::BTreeSet<[u8; 32]> =
        manifest.hash_sequence().into_iter().collect();
    allowed.insert(manifest.sequence_root());
    allowed
}

/// The blobs and packs a receiver must fetch. An empty file is written without a fetch, so its
/// root is not one; a folder of empty files falls back to waiting for the sequence itself.
pub(in crate::engine) fn content_roots(manifest: &Manifest) -> Vec<[u8; 32]> {
    let mut roots: Vec<[u8; 32]> = manifest.packs().iter().map(|pack| pack.root).collect();
    let mut seen: std::collections::BTreeSet<[u8; 32]> = roots.iter().copied().collect();
    for entry in manifest.entries() {
        if entry.kind == Kind::File
            && entry.size > 0
            && let Loc::Blob { root } = entry.loc
            && seen.insert(root)
        {
            roots.push(root);
        }
    }
    roots
}
