//! What a confined send copies into its store, and the checks that come first: the room for it, that
//! the file is the object the walk saw, and that no more is read than the walk saw.

use std::fs;

use crate::engine::storage;
use crate::manifest::{Below, Built};
use crate::{Error, Home};

/// Refuses a send whose copies would not fit where the home keeps them. Only what a confined walk
/// found is copied; a send that references its files in place copies none of them.
pub(super) fn check_room(home: &Home, built: &Built) -> Result<(), Error> {
    let manifest = &built.manifest;
    let needed: u64 = manifest
        .entries()
        .iter()
        .enumerate()
        .filter(|(index, _)| built.disk.below(*index).is_some())
        .map(|(_, entry)| entry.size)
        .fold(0, u64::saturating_add);
    if needed == 0 {
        return Ok(());
    }
    let folder = home.temporary_folder()?;
    let available = storage::available_bytes(&folder)?;
    if available < needed {
        return Err(Error::Storage(format!(
            "the home's volume has {available} bytes free and this send copies {needed} bytes into \
             it; free some space, or send less"
        )));
    }
    Ok(())
}

/// What a confined copy says when the file is not what the walk read.
pub(super) const CHANGED: &str = "a file changed after the folder was read, so it is not sent";

/// What it says when another object has taken the file's name.
pub(super) const REPLACED: &str =
    "a file was replaced after the folder was read, so it is not sent";

/// Opens a confined file again for copying, and checks it is the object the walk saw and still has
/// the size and time the manifest recorded.
pub(super) fn open_for_copy(
    below: &Below,
    entry: &crate::manifest::Entry,
) -> Result<fs::File, Error> {
    let file = below
        .open()
        .map_err(|_| Error::Verify(REPLACED.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| Error::Storage(format!("cannot read a file: {}", e.kind())))?;
    if meta.len() != entry.size || crate::manifest::mtime_ms_of(&meta) != entry.mtime_ms {
        return Err(Error::Verify(CHANGED.to_string()));
    }
    Ok(file)
}

/// The bytes of an open file in chunks, read on a blocking thread so the runtime keeps its workers.
/// It reads no more than `size` plus one byte: a file that grew ends the stream with an error, and
/// so does one that came up short.
pub(super) fn chunks(
    file: fs::File,
    size: u64,
) -> impl n0_future::Stream<Item = std::io::Result<bytes::Bytes>> + Send + Sync + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        let mut reader = std::io::Read::take(file, size.saturating_add(1));
        let mut total = 0u64;
        loop {
            let mut buffer = vec![0u8; 1 << 20];
            match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    total += n as u64;
                    if total > size {
                        let _ = tx.blocking_send(Err(std::io::Error::other(CHANGED)));
                        return;
                    }
                    buffer.truncate(n);
                    if tx.blocking_send(Ok(bytes::Bytes::from(buffer))).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = tx.blocking_send(Err(e));
                    return;
                }
            }
        }
        if total != size {
            let _ = tx.blocking_send(Err(std::io::Error::other(CHANGED)));
        }
    });
    n0_future::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
}

/// Reads a packed confined file whole, never more than its size plus one byte.
pub(super) fn read_packed(file: fs::File, size: u64) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(file, size.saturating_add(1)),
        &mut body,
    )
    .map_err(|e| Error::Storage(format!("cannot read a packed file: {}", e.kind())))?;
    if body.len() as u64 != size {
        return Err(Error::Verify(CHANGED.to_string()));
    }
    Ok(body)
}
