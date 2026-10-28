//! One byte range copied out of the store into the output.

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::mpsc::UnboundedSender,
};

use super::super::{Event, ReceiveOptions, storage};
use super::file::{self, Meta};
use crate::Error;

/// Writes `len` bytes at `offset` of a blob in the store into `target`. The fetch that put those
/// blocks in the store verified them against the root, so this only copies and installs them.
#[allow(clippy::too_many_arguments)]
pub(in crate::engine) async fn write_range(
    store: &FsStore,
    hash: Hash,
    offset: u64,
    len: u64,
    target: &std::path::Path,
    mtime_ms: u64,
    mode: u32,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let meta = Meta { mtime_ms, mode };
    let tmp = storage::temp_dir(&options.out)?;
    let incoming = storage::temp_path(tmp.path(), 0);
    let mut reader = store.blobs().reader(hash);
    reader
        .seek(std::io::SeekFrom::Start(offset))
        .await
        .map_err(|e| storage::storage("read", target, e))?;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&incoming)
        .await
        .map_err(|e| storage::storage("create", &incoming, e))?;
    let copied = tokio::io::copy(&mut reader.take(len), &mut file)
        .await
        .map_err(|e| storage::storage("write", &incoming, e))?;
    file.flush()
        .await
        .map_err(|e| storage::storage("write", &incoming, e))?;
    let file = file.into_std().await;
    if copied != len {
        return Err(Error::Verify(format!(
            "the store gave {copied} bytes where the manifest says {len}"
        )));
    }
    file::apply(&file, &incoming, meta, options.no_times, events)?;
    file.sync_all()
        .map_err(|e| storage::storage("sync", &incoming, e))?;
    drop(file);
    storage::install(&incoming, target, options.force)
}
