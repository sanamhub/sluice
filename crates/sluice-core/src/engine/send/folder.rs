//! Folders and several paths from the sender's side. The manifest builder has already read and
//! hashed every file; this module puts the same bytes into the home store as the blobs and packs
//! the manifest names, then starts one provider for the whole sequence.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::{collections::BTreeSet, fs, path::PathBuf};

use iroh_blobs::{
    BlobFormat,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};

use super::copy::{check_room, chunks, open_blocking, read_packed, stopped};
use super::{Congestion, Outgoing, Provider, RelayModeChoice, Scratch, serving, storage};
use crate::{
    Error, Home,
    manifest::{Build, Built, ConfinedArg, Kind, Loc, Manifest},
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
    let built = Manifest::build_paths(paths)?;
    let never = Arc::new(AtomicBool::new(false));
    prepare_built(home, built, relay, congestion, expiry_ms, port, rate, never).await
}

/// [`prepare_paths_with_rate`] for a caller that opened and proved its folders itself, such as a
/// daemon's roots. The walk lists and opens everything relative to the handles it is given and never
/// follows a link, so nothing outside them can be sent. Every file is then opened again the same
/// way, and the file found is accepted only if it is the object the walk saw there, at the hash and
/// again at the copy, so the walk, the hash and the copy cannot disagree and nothing is read from a
/// path afterwards. `cancel` is checked between files and between chunks, and a send that is told to
/// stop ends with [`Error::Cancelled`] and removes its store. A one-shot `send` does not do this: it
/// references its files where they live.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_paths_confined(
    home: &Home,
    args: Vec<ConfinedArg>,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
    cancel: Arc<AtomicBool>,
) -> Result<Outgoing, Error> {
    // The walk and the hashing read every file, which is not work for an async thread.
    let built = tokio::task::spawn_blocking(move || Manifest::build_confined(args))
        .await
        .map_err(|_| Error::Bug("the walk did not finish".to_string()))??;
    prepare_built(
        home, built, relay, congestion, expiry_ms, port, rate, cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_built(
    home: &Home,
    built: Build,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
    cancel: Arc<AtomicBool>,
) -> Result<Outgoing, Error> {
    // Read before the store exists, so a home whose identity cannot be read fails this send with
    // nothing of its own left behind.
    let seed = home.identity()?.secret_seed();
    let Build::Ready(built) = built else {
        return Err(Error::Policy(
            "a path cannot be sent because a receiver would reject one of its names".to_string(),
        ));
    };
    let manifest = &built.manifest;
    check_room(home, &built)?;
    let scratch = Scratch::open(home, storage::Collector::Off).await?;
    let mut tags = Vec::new();
    let filled = fill(&scratch, &built, &mut tags, &cancel).await;
    let sequence_root = match filled {
        Ok(root) => root,
        // A send that fails while it copies leaves nothing of its own: the store is closed and its
        // folder removed here, and not handed to a later command.
        Err(e) => {
            drop(tags);
            let _ = scratch.close().await;
            return Err(e);
        }
    };
    let mut allowed: BTreeSet<[u8; 32]> = manifest.hash_sequence().into_iter().collect();
    allowed.insert(sequence_root);
    let provider = serving(
        home, scratch, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    // Offered before the ticket is printed, so a receiver that has the ticket finds the offer.
    provider.offer_receipts(sequence_root, total_bytes(manifest), seed, expiry_ms);
    let ticket = match ticket_of(&provider, manifest, sequence_root, expiry_ms) {
        Ok(ticket) => ticket,
        Err(e) => {
            // Stopping the provider closes the store and removes the folder with it.
            let _ = provider.shutdown().await;
            return Err(e);
        }
    };
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

/// Puts everything a send serves into its store, and returns the root of the hash sequence.
async fn fill(
    scratch: &Scratch,
    built: &Built,
    tags: &mut Vec<TempTag>,
    cancel: &Arc<AtomicBool>,
) -> Result<[u8; 32], Error> {
    let manifest = &built.manifest;
    import_blobs(scratch.store(), built, manifest, tags, cancel).await?;
    for index in 0..manifest.packs().len() {
        add_pack(scratch.store(), built, manifest, index, tags, cancel).await?;
    }
    add_manifest(scratch.store(), manifest, tags).await?;
    add_sequence(scratch.store(), manifest, tags).await
}

/// Imports every distinct file blob once, in place. A second file with the same content shares the
/// blob, which is why the sequence names each root once.
async fn import_blobs(
    store: &FsStore,
    built: &Built,
    manifest: &Manifest,
    tags: &mut Vec<TempTag>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), Error> {
    let mut done = BTreeSet::new();
    for (index, entry) in manifest.entries().iter().enumerate() {
        let Loc::Blob { root } = entry.loc else {
            continue;
        };
        if stopped(cancel) {
            return Err(Error::Cancelled);
        }
        if !done.insert(root) {
            continue;
        }
        let disk = built
            .disk
            .file(index)
            .ok_or_else(|| Error::Bug("a file entry has no disk path".to_string()))?;
        let added = match built.disk.below(index) {
            // A confined send copies what it read from the handle it proved, so a later change to
            // the path cannot put other bytes on the wire.
            Some(below) => {
                let file = open_blocking(below, entry).await?;
                store
                    .blobs()
                    .add_stream(chunks(file, entry.size, cancel.clone()))
                    .await
                    .temp_tag()
                    .await
            }
            None => {
                store
                    .blobs()
                    .add_path_with_opts(AddPathOptions {
                        path: disk.to_path_buf(),
                        format: BlobFormat::Raw,
                        mode: ImportMode::TryReference,
                    })
                    .temp_tag()
                    .await
            }
        };
        let tag = added.map_err(|e| {
            if stopped(cancel) {
                Error::Cancelled
            } else {
                Error::Storage(format!("cannot import a file: {}", crate::plain(&e)))
            }
        })?;
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
    cancel: &Arc<AtomicBool>,
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
        if stopped(cancel) {
            return Err(Error::Cancelled);
        }
        let disk = built
            .disk
            .file(entry_index)
            .ok_or_else(|| Error::Bug("a packed entry has no disk path".to_string()))?;
        let body = match built.disk.below(entry_index) {
            Some(below) => {
                let file = open_blocking(below, entry).await?;
                let (size, cancel) = (entry.size, cancel.clone());
                tokio::task::spawn_blocking(move || read_packed(file, size, &cancel))
                    .await
                    .map_err(|_| Error::Bug("the read of a file did not finish".to_string()))??
            }
            None => fs::read(disk)
                .map_err(|e| Error::Storage(format!("cannot read a packed file: {}", e.kind())))?,
        };
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

#[cfg(test)]
async fn add_pack_for_test(built: &Built) -> Result<(), Error> {
    let cancel = Arc::new(AtomicBool::new(false));
    let dir = tempfile::tempdir().unwrap();
    let store = FsStore::load(dir.path()).await.unwrap();
    let mut tags = Vec::new();
    let result = add_pack(&store, built, &built.manifest, 0, &mut tags, &cancel).await;
    drop(tags);
    store.shutdown().await.unwrap();
    result
}

#[cfg(test)]
mod tests;
