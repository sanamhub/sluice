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

use super::{Congestion, Outgoing, Provider, RelayModeChoice, Stamp, storage};
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
    let store = FsStore::load(home.path().join("blobs"))
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store: {e}")))?;
    let tag = import(path, &shown, &store).await?;
    let hash = tag.hash();
    if !before.same(path)? {
        return Err(Error::Verify(format!(
            "{shown} changed while it was read, so its root names no version of it"
        )));
    }
    let manifest = one_entry_manifest(entry_path, &before, hash)?;
    keep_metadata(&store, &manifest).await?;
    let relay_text = relay_text(&relay);
    let allowed = allowed_roots(&manifest);
    let provider = Provider::start_with_rate(
        home, store, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    let ticket = ticket_of(&provider, &manifest, before.size, expiry_ms, relay_text)?;
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
        _tags: vec![tag],
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
    let store = FsStore::load(home.path().join("blobs"))
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store: {e}")))?;
    let (tag, size) = import_stdin(home, &store).await?;
    let hash = tag.hash();
    let allowed = std::collections::BTreeSet::from([*hash.as_bytes()]);
    let relay_text = relay_text(&relay);
    let provider = Provider::start_with_rate(
        home, store, allowed, relay, congestion, expiry_ms, port, rate,
    )
    .await?;
    let ticket = ticket_with_body(
        &provider,
        expiry_ms,
        relay_text,
        Body::Static {
            root: *hash.as_bytes(),
            size,
            format: Format::Raw,
        },
    )?;
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

/// Reads standard input into a private file and copies it into the store, so the pipe closes
/// before the ticket exists and the bytes survive the reader going away.
async fn import_stdin(home: &Home, store: &FsStore) -> Result<(TempTag, u64), Error> {
    let tmp_root = home.path().join("tmp");
    fs::create_dir_all(&tmp_root)
        .map_err(|e| Error::Storage(format!("cannot create {}: {e}", tmp_root.display())))?;
    let tmp = storage::temp_dir(&tmp_root)?;
    let path = tmp.join("input");
    let size = match read_stdin(&path).await {
        Ok(size) => size,
        Err(e) => {
            storage::remove_quietly(&tmp);
            return Err(e);
        }
    };
    let copied = store
        .blobs()
        .add_path_with_opts(AddPathOptions {
            path: path.clone(),
            format: BlobFormat::Raw,
            mode: ImportMode::Copy,
        })
        .temp_tag()
        .await;
    storage::remove_quietly(&tmp);
    let tag = copied
        .map_err(|e| Error::Storage(format!("cannot keep standard input: {}", crate::chain(&e))))?;
    Ok((tag, size))
}

/// Reads standard input to a file on disk, so a large pipe never sits in memory.
async fn read_stdin(path: &Path) -> Result<u64, Error> {
    let target = path.to_path_buf();
    let read = tokio::task::spawn_blocking(move || {
        let mut file = fs::File::create(&target)?;
        let mut stdin = std::io::stdin().lock();
        let size = std::io::copy(&mut stdin, &mut file)?;
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
        .map_err(|e| Error::Storage(format!("cannot import {shown}: {}", crate::chain(&e))))
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
async fn keep_metadata(store: &FsStore, manifest: &Manifest) -> Result<(), Error> {
    let added = store
        .blobs()
        .add_bytes(manifest.encode())
        .await
        .map_err(|e| Error::Storage(format!("cannot keep the manifest: {}", crate::chain(&e))))?;
    if added.hash != Hash::from(manifest.root()) {
        return Err(Error::Bug(
            "the store gave the manifest a root other than its BLAKE3 hash".to_string(),
        ));
    }
    let added = store
        .blobs()
        .add_bytes(manifest.sequence_bytes())
        .await
        .map_err(|e| {
            Error::Storage(format!(
                "cannot keep the hash sequence: {}",
                crate::chain(&e)
            ))
        })?;
    if added.hash != Hash::from(manifest.sequence_root()) {
        return Err(Error::Bug(
            "the store gave the hash sequence a root other than its BLAKE3 hash".to_string(),
        ));
    }
    Ok(())
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
