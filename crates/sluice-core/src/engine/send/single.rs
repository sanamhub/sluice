//! One regular file, or standard input, from the sender's side: import it, build the one-entry
//! manifest (or none, for a raw pipe), and prepare the ticket.

use std::{fs, path::Path};

use iroh_blobs::{
    BlobFormat, Hash,
    api::{
        TempTag,
        blobs::{AddPathOptions, ImportMode},
    },
    store::fs::FsStore,
};

use super::{Congestion, Outgoing, Provider, RelayModeChoice, Scratch, Stamp, storage};
use crate::{
    Error, Home,
    manifest::{Entry, Kind, Loc, Manifest, PackRef, path::ManifestPath},
    ticket::{Body, Format, Ticket},
};

/// Imports `path` in place and starts a provider for it. `expiry` is UTC milliseconds or none.
///
/// Fails with [`Error::Policy`] for a folder, a symlink or a name a receiver would reject, and
/// with [`Error::Verify`] if the file changes between the import and the start of serving.
pub async fn prepare(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
) -> Result<Outgoing, Error> {
    prepare_with_rate(home, path, relay, congestion, expiry_ms, port, None).await
}

/// [`prepare`] with a byte rate for serving the file.
pub async fn prepare_with_rate(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let (shown, name, entry_path, before) = checked_file(path)?;
    let scratch = Scratch::open(home, storage::Collector::Off).await?;
    let tag = import(path, &shown, scratch.store()).await?;
    let hash = tag.hash();
    if !before.same(path)? {
        return Err(Error::Verify(format!(
            "{shown} changed while it was read, so its root names no version of it"
        )));
    }
    let manifest = one_entry_manifest(entry_path, &before, hash)?;
    let metadata = keep_metadata(scratch.store(), &manifest).await?;
    let relay_text = relay_text(&relay);
    let allowed = allowed_roots(&manifest);
    let provider = match Provider::start_with_rate(
        home,
        scratch.store().handle(),
        allowed,
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
    {
        Ok(provider) => provider,
        Err(e) => {
            // The send's folder goes with the failure, so nothing it copied stays in the home.
            let _ = scratch.close().await;
            return Err(e);
        }
    };
    // The provider closes the store and then removes this folder, on every way it can end.
    let Some(dir) = scratch.release() else {
        return Err(Error::Bug("the send's folder is gone".to_string()));
    };
    let mut provider = provider;
    provider.hold(dir);
    let ticket = match ticket_of(&provider, &manifest, before.size, expiry_ms, relay_text) {
        Ok(ticket) => ticket,
        Err(e) => {
            // Stopping the provider closes the store and removes the folder with it.
            let _ = provider.shutdown().await;
            return Err(e);
        }
    };
    // The ticket is readable by anyone who has it, so it is printed and never traced.
    Ok(Outgoing {
        provider,
        ticket,
        roots: content_roots(&manifest),
        name: Some(name),
        files: 1,
        bytes: before.size,
        congestion,
        live: None,
        _tags: {
            let mut tags = vec![tag];
            tags.extend(metadata);
            tags
        },
    })
}

/// Sends standard input as a raw blob, ticket format 0. The bytes are read to the end before the
/// root is known, so they are copied into the store rather than referenced, and the pipe closes
/// before the ticket is printed.
pub async fn prepare_stdin(
    home: &Home,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
) -> Result<Outgoing, Error> {
    prepare_stdin_with_rate(home, relay, congestion, expiry_ms, port, None).await
}

/// [`prepare_stdin`] with a byte rate for serving the blob.
pub async fn prepare_stdin_with_rate(
    home: &Home,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    prepare_stdin_body(
        home,
        std::io::stdin(),
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
}

/// [`prepare_stdin_with_rate`] with the stream to read as an argument, so a test sends bytes without
/// a pipe. The bytes are read to the end before the root is known, so they are copied into the
/// send's own store, and the stream is closed before the ticket exists.
async fn prepare_stdin_body(
    home: &Home,
    from: impl std::io::Read + Send + 'static,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    rate: Option<std::sync::Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let scratch = Scratch::open(home, storage::Collector::Off).await?;
    let (tag, size) = import_stdin(scratch.path(), scratch.store(), from).await?;
    let hash = tag.hash();
    let allowed = std::collections::BTreeSet::from([*hash.as_bytes()]);
    let relay_text = relay_text(&relay);
    let provider = match Provider::start_with_rate(
        home,
        scratch.store().handle(),
        allowed,
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await
    {
        Ok(provider) => provider,
        Err(e) => {
            let _ = scratch.close().await;
            return Err(e);
        }
    };
    // The provider closes the store and then removes this folder, on every way it can end.
    let Some(dir) = scratch.release() else {
        return Err(Error::Bug("the send's folder is gone".to_string()));
    };
    let mut provider = provider;
    provider.hold(dir);
    let ticket = match ticket_with_body(
        &provider,
        expiry_ms,
        relay_text,
        Body::Static {
            root: *hash.as_bytes(),
            size,
            format: Format::Raw,
        },
    ) {
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
        roots: vec![*hash.as_bytes()],
        name: None,
        files: 1,
        bytes: size,
        congestion,
        live: None,
        _tags: vec![tag],
    })
}

/// Reads standard input into a file of the send's own folder and copies it into the store, so the
/// pipe closes before the ticket exists and the bytes survive the reader going away. The file is
/// removed once the store holds the bytes, so the folder holds one copy of them.
async fn import_stdin(
    dir: &Path,
    store: &FsStore,
    from: impl std::io::Read + Send + 'static,
) -> Result<(TempTag, u64), Error> {
    let path = dir.join("input");
    let size = read_input(&path, from).await?;
    let copied = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.clone(),
            format: BlobFormat::Raw,
            mode: ImportMode::Copy,
        })
        .temp_tag()
        .await;
    let tag = copied
        .map_err(|e| Error::Storage(format!("cannot keep standard input: {}", crate::plain(&e))))?;
    let _ = fs::remove_file(&path);
    Ok((tag, size))
}

/// Reads a stream to a file on disk, so a large pipe never sits in memory.
async fn read_input(
    path: &Path,
    mut from: impl std::io::Read + Send + 'static,
) -> Result<u64, Error> {
    let target = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || {
        let mut file = fs::File::create(&target)?;
        let size = std::io::copy(&mut from, &mut file)?;
        file.sync_all()?;
        Ok::<u64, std::io::Error>(size)
    })
    .await
    .map_err(|e| Error::Storage(format!("reading standard input stopped: {e}")))?;
    read.map_err(|e| storage::storage("read standard input into", path, e))
}

/// A ticket for a body other than a manifest sequence, with the provider's addresses and relay.
fn ticket_with_body(
    provider: &Provider,
    expiry_ms: Option<u64>,
    relay_text: Option<String>,
    body: Body,
) -> Result<Ticket, Error> {
    let addr = provider.addr();
    let relay_url = relay_text.or_else(|| {
        addr.relay_urls()
            .next()
            .map(std::string::ToString::to_string)
    });
    Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        body,
    )
}
/// The name, path and metadata of the one file a single-file send carries.
fn checked_file(path: &Path) -> Result<(String, String, ManifestPath, Stamp), Error> {
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

async fn import(path: &Path, shown: &str, store: &FsStore) -> Result<TempTag, Error> {
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
fn one_entry_manifest(
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
async fn keep_metadata(store: &FsStore, manifest: &Manifest) -> Result<Vec<TempTag>, Error> {
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
fn allowed_roots(manifest: &Manifest) -> std::collections::BTreeSet<[u8; 32]> {
    let mut allowed: std::collections::BTreeSet<[u8; 32]> =
        manifest.hash_sequence().into_iter().collect();
    allowed.insert(manifest.sequence_root());
    allowed
}

fn relay_text(relay: &RelayModeChoice) -> Option<String> {
    match relay {
        RelayModeChoice::Disabled | RelayModeChoice::N0 => None,
        RelayModeChoice::Url(url) => Some(url.clone()),
    }
}

fn ticket_of(
    provider: &Provider,
    manifest: &Manifest,
    size: u64,
    expiry_ms: Option<u64>,
    relay_text: Option<String>,
) -> Result<Ticket, Error> {
    let addr = provider.addr();
    let relay_url = relay_text.or_else(|| {
        addr.relay_urls()
            .next()
            .map(std::string::ToString::to_string)
    });
    Ticket::new(
        *addr.id.as_bytes(),
        addr.ip_addrs().copied().collect(),
        relay_url,
        expiry_ms,
        Body::Static {
            root: manifest.sequence_root(),
            size,
            format: Format::HashSequence,
        },
    )
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

/// A send of standard input whose bytes come from memory rather than a pipe, for a test that has no
/// pipe to write to.
#[cfg(test)]
pub(crate) async fn prepare_stdin_from(home: &Home, bytes: Vec<u8>) -> Result<Outgoing, Error> {
    prepare_stdin_body(
        home,
        std::io::Cursor::new(bytes),
        RelayModeChoice::Disabled,
        Congestion::Cubic,
        None,
        None,
        None,
    )
    .await
}
