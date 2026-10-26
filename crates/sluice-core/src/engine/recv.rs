//! One file from the receiver's side: fetch the manifest, check the destination, fetch the bytes
//! into a per-transfer store beside it, then move the verified blob into place.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

use iroh_blobs::{
    Hash, HashAndFormat,
    api::{
        blobs::{ExportMode, ExportOptions},
        proto::BlobStatus,
        remote::GetProgressItem,
    },
    store::fs::FsStore,
};
use n0_future::StreamExt;
use tokio::sync::mpsc::UnboundedSender;

use super::{Event, Receiver, RelayModeChoice, emit, storage};
use crate::{
    Error,
    manifest::{Kind, Limits, Loc, Manifest},
    ticket::{Body, Format, Ticket},
};

/// Where and how a receive writes.
#[derive(Clone, Debug)]
pub struct ReceiveOptions {
    /// The folder to write into.
    pub out: PathBuf,
    /// Replace a path that is already there instead of refusing.
    pub force: bool,
    /// Do not restore the modification time.
    pub no_times: bool,
}

/// What arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// The file that was written.
    pub path: PathBuf,
    /// Its size in bytes.
    pub bytes: u64,
}

/// Receives the one file a static ticket names. A folder is refused until the folder manifest
/// writer lands; the ticket format is already the one it will use.
pub async fn receive(
    text: &str,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<Received, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    let root = match ticket.body() {
        Body::Static {
            root,
            format: Format::HashSequence,
            ..
        } => *root,
        Body::Static { .. } => {
            return Err(Error::Policy(
                "this ticket is a raw stream; receive it with --out - in a later change"
                    .to_string(),
            ));
        }
        Body::Live { .. } => {
            return Err(Error::Policy(
                "this is a live ticket; live receive is a later change".to_string(),
            ));
        }
    };
    fs::create_dir_all(&options.out)
        .map_err(|e| storage::policy_path("create", &options.out, e))?;
    let receiver = Receiver::open(relay_of(&ticket)).await?;
    let conn = receiver.connect(&ticket, events.clone()).await?;
    let partial = storage::partial_dir(&options.out, &hex(&root));
    fs::create_dir_all(&partial).map_err(|e| storage::storage("create", &partial, e))?;
    let store = FsStore::load(&partial)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the partial store: {e}")))?;

    let result = fetch_all(&ticket, &store, conn, options, events, &partial).await;
    let closed = store.shutdown().await;
    match (result, closed) {
        (Ok(received), Ok(())) => {
            // The blob was moved out; what is left is metadata and the manifest.
            let _ = fs::remove_dir_all(&partial);
            Ok(received)
        }
        (Err(e), _) => {
            // Deliberately keep the partial store: a second receive continues from it.
            Err(e)
        }
        (Ok(_), Err(e)) => Err(Error::Storage(format!(
            "the file is in place but the partial store did not close: {e}"
        ))),
    }
}

async fn fetch_all(
    ticket: &Ticket,
    store: &FsStore,
    conn: iroh::endpoint::Connection,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
    _partial: &Path,
) -> Result<Received, Error> {
    let manifest_root = match ticket.body() {
        Body::Static { root, .. } => *root,
        Body::Live { .. } => return Err(Error::Policy("not a static ticket".to_string())),
    };
    fetch_blob(store, &conn, Hash::from(manifest_root), 0, events).await?;
    let (manifest_bytes, manifest_size) = complete_bytes(store, Hash::from(manifest_root)).await?;
    let limits = Limits::default();
    limits.check_bytes(manifest_size)?;
    let manifest = Manifest::decode(&manifest_bytes, limits)?;
    let entry = only_file(&manifest)?;
    let target = options.out.join(entry.path.as_str());
    check_target(&target, options.force)?;
    check_space(&options.out, ticket, entry.size)?;

    emit(
        events,
        Event::Started {
            name: Some(entry.path.as_str().to_string()),
            files: 1,
            bytes: entry.size,
        },
    );
    let content = content_root(&manifest, entry.loc)?;
    let started = Instant::now();
    fetch_blob(store, &conn, content, entry.size, events).await?;
    let (_, size) = complete_bytes(store, content).await?;
    if size != entry.size {
        return Err(Error::Verify(format!(
            "the blob has {size} bytes and the manifest says {}",
            entry.size
        )));
    }

    let incoming = storage::incoming_path(&target);
    store
        .blobs()
        .export_with_opts(ExportOptions {
            hash: content,
            mode: ExportMode::TryReference,
            target: incoming.clone(),
        })
        .finish()
        .await
        .map_err(|e| Error::Storage(format!("cannot write {}: {e}", target.display())))?;
    storage::sync_file(&incoming)?;
    storage::install(&incoming, &target, options.force)?;
    if !options.no_times {
        set_time(&target, entry.mtime_ms)?;
    }
    set_mode(&target, entry.mode)?;
    emit(events, Event::Verified { bytes: entry.size });
    emit(
        events,
        Event::Done {
            bytes: entry.size,
            seconds: started.elapsed().as_secs_f64(),
        },
    );
    Ok(Received {
        path: target,
        bytes: entry.size,
    })
}

fn only_file(manifest: &Manifest) -> Result<&crate::manifest::Entry, Error> {
    let mut files = manifest.entries().iter().filter(|e| e.kind == Kind::File);
    let Some(entry) = files.next() else {
        return Err(Error::Policy(
            "the manifest names no file, so there is nothing to write".to_string(),
        ));
    };
    if files.next().is_some() {
        return Err(Error::Policy(
            "the manifest names more than one file; folder receive is a later change".to_string(),
        ));
    }
    Ok(entry)
}

fn content_root(manifest: &Manifest, loc: Loc) -> Result<Hash, Error> {
    match loc {
        Loc::Blob { root } => Ok(Hash::from(root)),
        Loc::Packed {
            pack_index,
            offset: 0,
            len,
        } => {
            let pack = manifest.packs().get(pack_index as usize).ok_or_else(|| {
                Error::Policy("the manifest points at a pack that is not listed".to_string())
            })?;
            if pack.size != len {
                return Err(Error::Policy(
                    "the file is one part of a pack; folder receive is a later change".to_string(),
                ));
            }
            Ok(Hash::from(pack.root))
        }
        Loc::Packed { .. } => Err(Error::Policy(
            "the file is one part of a pack; folder receive is a later change".to_string(),
        )),
        Loc::None => Err(Error::Policy(
            "the manifest names a folder or link as a file".to_string(),
        )),
    }
}

fn refuse_expired(ticket: &Ticket) -> Result<(), Error> {
    if ticket.is_expired(super::now_ms()) {
        return Err(Error::Policy(
            "the ticket has expired; ask the sender for a new one".to_string(),
        ));
    }
    Ok(())
}

fn relay_of(ticket: &Ticket) -> RelayModeChoice {
    ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    })
}

fn check_target(target: &Path, force: bool) -> Result<(), Error> {
    if force {
        return Ok(());
    }
    match fs::symlink_metadata(target) {
        Ok(_) => Err(storage::clash(target)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(storage::policy_path("check", target, e)),
    }
}

fn check_space(out: &Path, ticket: &Ticket, size: u64) -> Result<(), Error> {
    let available = storage::available_bytes(out)?;
    let needed = storage::needed_bytes(size);
    if available < needed {
        return Err(Error::Storage(format!(
            "{} has {available} bytes free and this transfer needs about {needed}",
            out.display()
        )));
    }
    let promised = match ticket.body() {
        Body::Static { size, .. } => *size,
        Body::Live { .. } => 0,
    };
    if promised != size {
        return Err(Error::Policy(format!(
            "the ticket promises {promised} bytes and the manifest says {size}"
        )));
    }
    Ok(())
}

async fn complete_bytes(store: &FsStore, hash: Hash) -> Result<(Vec<u8>, u64), Error> {
    let status = store
        .blobs()
        .status(hash)
        .await
        .map_err(|e| Error::Storage(format!("cannot read the partial store: {e}")))?;
    match status {
        BlobStatus::Complete { size } => {
            let bytes = store
                .blobs()
                .get_bytes(hash)
                .await
                .map_err(|e| Error::Storage(format!("cannot read the received blob: {e}")))?;
            Ok((bytes.to_vec(), size))
        }
        _ => Err(Error::Verify(
            "the transfer ended without the whole blob".to_string(),
        )),
    }
}

async fn fetch_blob(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    hash: Hash,
    total: u64,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let mut progress = std::pin::pin!(
        store
            .remote()
            .fetch(conn.clone(), HashAndFormat::raw(hash))
            .stream()
    );
    while let Some(item) = progress.next().await {
        match item {
            GetProgressItem::Progress(bytes) => {
                if total > 0 {
                    emit(events, Event::Progress { bytes, total });
                }
            }
            GetProgressItem::Done(_) => return Ok(()),
            GetProgressItem::Error(e) => {
                let text = e.to_string();
                return Err(if text.contains("hash") && text.contains("mismatch") {
                    Error::Verify("received bytes did not match the ticket root".to_string())
                } else {
                    Error::Network(format!("the transfer stopped: {text}"))
                });
            }
        }
    }
    Err(Error::Network(
        "the connection closed before the blob completed".to_string(),
    ))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Error> {
    use std::os::unix::fs::PermissionsExt;
    if mode == 0 {
        return Ok(());
    }
    // Setuid and setgid are never applied, whatever the sender wrote.
    let mode = mode & 0o777;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(windows)]
fn set_mode(path: &Path, mode: u32) -> Result<(), Error> {
    if mode == 0 {
        return Ok(());
    }
    let mut permissions = fs::metadata(path)
        .map_err(|e| storage::policy_path("read the mode of", path, e))?
        .permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, permissions)
        .map_err(|e| storage::policy_path("set the mode of", path, e))
}

#[cfg(not(any(unix, windows)))]
fn set_mode(_path: &Path, _mode: u32) -> Result<(), Error> {
    Ok(())
}

fn set_time(path: &Path, mtime_ms: u64) -> Result<(), Error> {
    if mtime_ms == 0 {
        return Ok(());
    }
    let time = UNIX_EPOCH + Duration::from_millis(mtime_ms);
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| storage::storage("open", path, e))?;
    file.set_times(fs::FileTimes::new().set_modified(time))
        .map_err(|e| storage::storage("set the time of", path, e))
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::storage::needed_bytes;

    #[test]
    fn the_space_check_is_for_one_copy() {
        let need = needed_bytes(100 << 30);
        assert!(need < 101 << 30);
    }
}
