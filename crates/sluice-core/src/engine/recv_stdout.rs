//! Receiving to standard output: a raw blob, or a transfer whose manifest holds one file. A folder
//! is refused, because a stream has no names. Nothing but content bytes goes to standard output.

use std::path::PathBuf;

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

/// Writes the bytes a ticket names to standard output and returns how many. The store is a
/// temporary one that is removed on the way out; nothing is written beside a destination folder.
pub async fn receive_stdout(
    text: &str,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let tmp = Temp(storage::temp_dir(&std::env::temp_dir())?);
    let store = match FsStore::load(tmp.0.join("store")).await {
        Ok(store) => store,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::chain(&e)
            )));
        }
    };
    let conn = receiver.connect(&ticket, events.clone()).await?;
    let result = stream(&conn, &ticket, &store, options, events).await;
    let _ = receiver.shutdown().await;
    let _ = store.shutdown().await;
    result
}

async fn stream(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    match ticket.body() {
        Body::Static {
            root,
            size,
            format: Format::Raw,
            ..
        } => {
            let hash = Hash::from(*root);
            fetch::fetch_whole(store, conn, hash).await?;
            let have = fetch::complete_size(store, hash).await?;
            if have != *size {
                return Err(Error::Verify(format!(
                    "the blob has {have} bytes and the ticket says {size}"
                )));
            }
            copy(store, hash, 0, *size).await?;
            Ok(*size)
        }
        Body::Static {
            format: Format::HashSequence,
            ..
        } => {
            let manifest = fetch_manifest(conn, ticket, store, options.limits, events).await?;
            let entry = single_file(&manifest)?;
            let bytes = entry.size;
            stream_entry(store, conn, &manifest, entry, events).await?;
            Ok(bytes)
        }
        Body::Live { .. } => Err(Error::Policy(
            "this is a live ticket, not supported yet".to_string(),
        )),
    }
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

async fn stream_entry(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    manifest: &Manifest,
    entry: &crate::manifest::Entry,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    match entry.loc {
        Loc::Blob { root } => {
            let hash = Hash::from(root);
            if entry.size > 0 {
                fetch::content_blob(store, conn, hash, entry.size, 0, entry.size, events).await?;
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

/// The temporary store, removed however the write ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        storage::remove_quietly(&self.0);
    }
}
