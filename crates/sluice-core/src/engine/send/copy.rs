//! What a confined send copies into its store, and the checks that come first: the room for it, that
//! the file is the object the walk saw, and that no more is read than the walk saw.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    refuse_without_room(storage::available_bytes(&folder)?, needed)
}

/// The decision of [`check_room`], with the free space given as a value so a test can try any
/// reading of it.
pub(super) fn refuse_without_room(available: u64, needed: u64) -> Result<(), Error> {
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
pub(super) fn open_for_copy(below: &Below, size: u64, mtime_ms: u64) -> Result<fs::File, Error> {
    #[cfg(test)]
    run_hook();
    let file = below
        .open()
        .map_err(|_| Error::Verify(REPLACED.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| Error::Storage(format!("cannot read a file: {}", e.kind())))?;
    if meta.len() != size || crate::manifest::mtime_ms_of(&meta) != mtime_ms {
        return Err(Error::Verify(CHANGED.to_string()));
    }
    Ok(file)
}

/// [`open_for_copy`] on a blocking thread, which is where the open and the first look at the file
/// belong.
pub(super) async fn open_blocking(
    below: &Below,
    entry: &crate::manifest::Entry,
) -> Result<fs::File, Error> {
    let (below, size, mtime) = (below.clone(), entry.size, entry.mtime_ms);
    tokio::task::spawn_blocking(move || open_for_copy(&below, size, mtime))
        .await
        .map_err(|_| Error::Bug("the open of a file did not finish".to_string()))?
}

/// What a copy that was told to stop says.
pub(super) const STOPPED: &str = "the send was stopped while it copied";

/// Whether the send was told to stop.
pub(super) fn stopped(cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed)
}

/// The bytes of an open file in chunks, read on a blocking thread so the runtime keeps its workers.
/// It reads no more than `size` plus one byte: a file that grew ends the stream with an error, and
/// so does one that came up short.
pub(super) fn chunks(
    file: fs::File,
    size: u64,
    cancel: Arc<AtomicBool>,
) -> impl n0_future::Stream<Item = std::io::Result<bytes::Bytes>> + Send + Sync + 'static {
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    tokio::task::spawn_blocking(move || {
        let mut reader = std::io::Read::take(file, size.saturating_add(1));
        let mut total = 0u64;
        loop {
            if stopped(&cancel) {
                let _ = tx.blocking_send(Err(std::io::Error::other(STOPPED)));
                return;
            }
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

/// Reads a packed confined file whole, never more than its size plus one byte, checking for a stop
/// between chunks.
pub(super) fn read_packed(
    file: fs::File,
    size: u64,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, Error> {
    let mut reader = std::io::Read::take(file, size.saturating_add(1));
    let mut body = Vec::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        if stopped(cancel) {
            return Err(Error::Cancelled);
        }
        match std::io::Read::read(&mut reader, &mut buffer) {
            Ok(0) => break,
            Ok(n) => body.extend_from_slice(&buffer[..n]),
            Err(e) => {
                return Err(Error::Storage(format!(
                    "cannot read a packed file: {}",
                    e.kind()
                )));
            }
        }
    }
    if body.len() as u64 != size {
        return Err(Error::Verify(CHANGED.to_string()));
    }
    Ok(body)
}

#[cfg(test)]
type Hook = Box<dyn Fn() + Send>;

#[cfg(test)]
static HOOK: std::sync::Mutex<Option<Hook>> = std::sync::Mutex::new(None);

#[cfg(test)]
fn run_hook() {
    if let Some(hook) = HOOK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
    {
        hook();
    }
}

/// Runs `hook` just before each confined file is opened for copying, where a test changes it. The
/// open is on a blocking thread, so the hook is shared and not thread-local.
#[cfg(test)]
pub(super) fn before_copy(hook: impl Fn() + Send + 'static) {
    *HOOK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(hook));
}

#[cfg(test)]
pub(super) fn clear_before_copy() {
    *HOOK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}
