//! Folders and several paths from the sender's side. The manifest builder has already read and
//! hashed every file; this module puts the same bytes into the home store as the blobs and packs
//! the manifest names, then starts one provider for the whole sequence.

use std::{collections::BTreeSet, fs, path::PathBuf};

use iroh_blobs::{
    BlobFormat,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};

use super::{Congestion, Outgoing, Provider, RelayModeChoice, storage};
use crate::{
    Error, Home,
    manifest::{Build, Built, Kind, Loc, Manifest},
    ticket::{Body, Format, Ticket},
};

/// Builds the manifest of `paths`, imports its files and packs, and starts the provider.
pub async fn prepare_paths(
    home: &Home,
    paths: &[PathBuf],
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
) -> Result<Outgoing, Error> {
    prepare_paths_with_rate(home, paths, relay, congestion, expiry_ms, port, None).await
}

/// [`prepare_paths`] with a byte rate for serving the tree.
pub async fn prepare_paths_with_rate(
    home: &Home,
    paths: &[PathBuf],
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let Build::Ready(built) = Manifest::build_paths(paths)? else {
        return Err(Error::Policy(
            "a path cannot be sent because a receiver would reject one of its names".to_string(),
        ));
    };
    let manifest = &built.manifest;
    let store = storage::open_store(&home.path().join("blobs"), storage::Collector::Off)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store: {e}")))?;
    let mut tags = Vec::new();
    import_blobs(&store, &built, manifest, &mut tags).await?;
    for index in 0..manifest.packs().len() {
        add_pack(&store, &built, manifest, index, &mut tags).await?;
    }
    add_manifest(&store, manifest, &mut tags).await?;
    let sequence_root = add_sequence(&store, manifest, &mut tags).await?;
    let mut allowed: BTreeSet<[u8; 32]> = manifest.hash_sequence().into_iter().collect();
    allowed.insert(sequence_root);
    let provider = Provider::start_with_rate(
        home, store, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    let ticket = ticket_of(&provider, manifest, sequence_root, expiry_ms)?;
    Ok(Outgoing {
        provider,
        ticket,
        roots: super::content_roots(manifest),
        name: None,
        files: file_count(manifest),
        bytes: total_bytes(manifest),
        congestion,
        live: None,
        _tags: tags,
    })
}

/// Imports every distinct file blob once, in place. A second file with the same content shares the
/// blob, which is why the sequence names each root once.
async fn import_blobs(
    store: &FsStore,
    built: &Built,
    manifest: &Manifest,
    tags: &mut Vec<TempTag>,
) -> Result<(), Error> {
    let mut done = BTreeSet::new();
    for (index, entry) in manifest.entries().iter().enumerate() {
        let Loc::Blob { root } = entry.loc else {
            continue;
        };
        if !done.insert(root) {
            continue;
        }
        let disk = built
            .disk
            .file(index)
            .ok_or_else(|| Error::Bug("a file entry has no disk path".to_string()))?;
        let tag = store
            .blobs()
            .add_path_with_opts(AddPathOptions {
                path: disk.to_path_buf(),
                format: BlobFormat::Raw,
                mode: ImportMode::TryReference,
            })
            .temp_tag()
            .await
            .map_err(|e| Error::Storage(format!("cannot import a file: {}", crate::plain(&e))))?;
        if *tag.hash().as_bytes() != root {
            return Err(Error::Verify(
                "a file changed between the manifest build and the import".to_string(),
            ));
        }
        tags.push(tag);
    }
    Ok(())
}

/// Reads the files of one pack in manifest order and adds the bytes as one blob. The pack is at
/// most `PACK_TARGET`, so holding one at a time is bounded.
async fn add_pack(
    store: &FsStore,
    built: &Built,
    manifest: &Manifest,
    index: usize,
    tags: &mut Vec<TempTag>,
) -> Result<(), Error> {
    let pack = manifest
        .packs()
        .get(index)
        .ok_or_else(|| Error::Bug("a pack index has no pack".to_string()))?;
    let mut bytes = Vec::with_capacity(pack.size as usize);
    for (entry_index, entry) in manifest.entries().iter().enumerate() {
        let Loc::Packed { pack_index, .. } = entry.loc else {
            continue;
        };
        if pack_index as usize != index {
            continue;
        }
        let disk = built
            .disk
            .file(entry_index)
            .ok_or_else(|| Error::Bug("a packed entry has no disk path".to_string()))?;
        let body = fs::read(disk)
            .map_err(|e| Error::Storage(format!("cannot read a packed file: {}", e.kind())))?;
        if body.len() as u64 != entry.size {
            return Err(Error::Verify(
                "a packed file changed size after the manifest was built".to_string(),
            ));
        }
        bytes.extend_from_slice(&body);
    }
    if bytes.len() as u64 != pack.size {
        return Err(Error::Bug(
            "a pack's files do not add up to its size".to_string(),
        ));
    }
    let tag = store
        .blobs()
        .add_bytes(bytes)
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot keep a pack: {}", crate::plain(&e))))?;
    if *tag.hash().as_bytes() != pack.root {
        return Err(Error::Verify(
            "a packed file changed between the manifest build and the import".to_string(),
        ));
    }
    tags.push(tag);
    Ok(())
}

/// Adds the manifest bytes and checks the store gave them the root the manifest computed.
async fn add_manifest(
    store: &FsStore,
    manifest: &Manifest,
    tags: &mut Vec<TempTag>,
) -> Result<(), Error> {
    let tag = store
        .blobs()
        .add_bytes(manifest.encode())
        .temp_tag()
        .await
        .map_err(|e| Error::Storage(format!("cannot keep the manifest: {}", crate::plain(&e))))?;
    if *tag.hash().as_bytes() != manifest.root() {
        return Err(Error::Bug(
            "the store gave the manifest a root other than its BLAKE3 hash".to_string(),
        ));
    }
    tags.push(tag);
    Ok(())
}

/// Adds the sequence blob and returns the root a ticket names.
async fn add_sequence(
    store: &FsStore,
    manifest: &Manifest,
    tags: &mut Vec<TempTag>,
) -> Result<[u8; 32], Error> {
    let tag = store
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
    let root = manifest.sequence_root();
    if *tag.hash().as_bytes() != root {
        return Err(Error::Bug(
            "the store gave the hash sequence a root other than its BLAKE3 hash".to_string(),
        ));
    }
    tags.push(tag);
    Ok(root)
}

fn ticket_of(
    provider: &Provider,
    manifest: &Manifest,
    sequence_root: [u8; 32],
    expiry_ms: Option<u64>,
) -> Result<Ticket, Error> {
    let addr = provider.addr();
    let relay_url = addr
        .relay_urls()
        .next()
        .map(std::string::ToString::to_string);
    Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        Body::Static {
            root: sequence_root,
            size: total_bytes(manifest),
            format: Format::HashSequence,
        },
    )
}

fn file_count(manifest: &Manifest) -> u64 {
    manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .count() as u64
}

fn total_bytes(manifest: &Manifest) -> u64 {
    manifest
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::File)
        .map(|entry| entry.size)
        .sum()
}
