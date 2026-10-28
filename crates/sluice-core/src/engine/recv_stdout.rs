//! Receiving to standard output: a raw blob, or a transfer whose manifest holds one file. A folder
//! is refused, because a stream has no names. Nothing but content bytes goes to standard output.

use std::path::Path;

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::mpsc::UnboundedSender,
};

use super::{
    Event, ReceiveOptions, Receiver, fetch,
    recv::{fetch_manifest, refuse_expired, relay_of},
    storage,
};
use crate::{
    Error,
    manifest::{Kind, Loc, Manifest},
    ticket::{Body, Format, Ticket},
};

/// Writes the bytes a ticket names to standard output and returns how many. The store is a private
/// temporary one under `store_root` (the home's `tmp`), checked for room before its body is
/// fetched; nothing is written beside a destination folder.
pub async fn receive_stdout(
    text: &str,
    options: &ReceiveOptions,
    store_root: &Path,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let tmp = match storage::temp_dir(store_root) {
        Ok(tmp) => tmp,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let store = match FsStore::load(tmp.path().join("store")).await {
        Ok(store) => store,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::plain(&e)
            )));
        }
    };
    let result = async {
        let conn = receiver.connect(&ticket, events.clone()).await?;
        stream(&conn, &ticket, &store, options, store_root, events).await
    }
    .await;
    let endpoint = receiver.shutdown().await;
    let closed = store.shutdown().await;
    // Removed before the result is returned, so no path out of this function leaves the folder.
    drop(tmp);
    let bytes = result?;
    if endpoint.is_err() || closed.is_err() {
        return Err(Error::Storage(
            "the bytes were written but the transfer did not close cleanly".to_string(),
        ));
    }
    Ok(bytes)
}

async fn stream(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    options: &ReceiveOptions,
    store_root: &Path,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    match ticket.body() {
        Body::Static {
            root,
            size,
            format: Format::Raw,
            ..
        } => raw(conn, store, *root, *size, store_root).await,
        Body::Static {
            format: Format::HashSequence,
            ..
        } => {
            let manifest = fetch_manifest(conn, ticket, store, options.limits, events).await?;
            let entry = single_file(&manifest)?;
            let needed = needed(&manifest, entry)?;
            storage::ensure_space(store_root, needed)?;
            let bytes = entry.size;
            stream_entry(
                store,
                conn,
                &manifest,
                entry,
                options.rate.as_deref(),
                events,
            )
            .await?;
            Ok(bytes)
        }
        Body::Live { .. } => Err(Error::Policy(
            "this is a live ticket, not supported yet".to_string(),
        )),
    }
}

/// A raw blob: its size is proved by its first chunk group, compared with the ticket, and checked
/// against the room under `store_root` before the body is asked for.
async fn raw(
    conn: &iroh::endpoint::Connection,
    store: &FsStore,
    root: [u8; 32],
    size: u64,
    store_root: &Path,
) -> Result<u64, Error> {
    let hash = Hash::from(root);
    if size > 0 {
        let proven = fetch::verified_size(store, conn, hash).await?;
        if proven != size {
            return Err(Error::Verify(format!(
                "the blob has {proven} bytes and the ticket says {size}"
            )));
        }
        storage::ensure_space(store_root, proven)?;
    }
    // Even no bytes are asked for as a whole blob, so the sender sees a request that reached the
    // root and can stop after one download.
    fetch::fetch_whole(store, conn, hash).await?;
    let have = fetch::complete_size(store, hash).await?;
    if have != size {
        return Err(Error::Verify(format!(
            "the blob has {have} bytes and the ticket says {size}"
        )));
    }
    copy(store, hash, 0, size).await?;
    Ok(size)
}

/// The one file a manifest may hold for a write to standard output.
fn single_file(manifest: &Manifest) -> Result<&crate::manifest::Entry, Error> {
    match manifest.entries() {
        [entry] if entry.kind == Kind::File => Ok(entry),
        _ => Err(Error::Policy(
            "a folder cannot be written to standard output; use --out DIR".to_string(),
        )),
    }
}

/// The bytes the store has to hold for this entry: the file itself, or the whole pack it is cut
/// from. A packed file is never fetched in pieces smaller than its pack.
fn needed(manifest: &Manifest, entry: &crate::manifest::Entry) -> Result<u64, Error> {
    match entry.loc {
        Loc::Blob { .. } => Ok(entry.size),
        Loc::Packed { pack_index, .. } => manifest
            .packs()
            .get(pack_index as usize)
            .map(|pack| pack.size)
            .ok_or_else(|| {
                Error::Policy("the manifest points at a pack that is not listed".into())
            }),
        Loc::None => Err(Error::Bug(
            "a File entry has no location to write".to_string(),
        )),
    }
}

async fn stream_entry(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    manifest: &Manifest,
    entry: &crate::manifest::Entry,
    rate: Option<&crate::engine::Rate>,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    match entry.loc {
        Loc::Blob { root } => {
            let hash = Hash::from(root);
            if entry.size > 0 {
                fetch::content_blob(store, conn, hash, entry.size, 0, entry.size, rate, events)
                    .await?;
            }
            copy(store, hash, 0, entry.size).await
        }
        Loc::Packed {
            pack_index,
            offset,
            len,
        } => {
            let pack = manifest.packs().get(pack_index as usize).ok_or_else(|| {
                Error::Policy("the manifest points at a pack that is not listed".to_string())
            })?;
            fetch::content_blob(
                store,
                conn,
                Hash::from(pack.root),
                pack.size,
                0,
                len,
                rate,
                events,
            )
            .await?;
            copy(store, Hash::from(pack.root), offset, len).await
        }
        Loc::None => Err(Error::Bug(
            "a File entry has no location to write".to_string(),
        )),
    }
}

/// Copies `len` bytes at `offset` of a blob in the store to standard output, and flushes it.
async fn copy(store: &FsStore, hash: Hash, offset: u64, len: u64) -> Result<(), Error> {
    if len == 0 {
        return Ok(());
    }
    let mut reader = store.blobs().reader(hash);
    if offset > 0 {
        reader
            .seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|e| storage::storage("read", std::path::Path::new("standard output"), e))?;
    }
    let mut out = tokio::io::stdout();
    let copied = tokio::io::copy(&mut reader.take(len), &mut out)
        .await
        .map_err(|e| storage::storage("write", std::path::Path::new("standard output"), e))?;
    out.flush()
        .await
        .map_err(|e| storage::storage("flush", std::path::Path::new("standard output"), e))?;
    if copied != len {
        return Err(Error::Verify(format!(
            "the store gave {copied} bytes where the manifest says {len}"
        )));
    }
    Ok(())
}
