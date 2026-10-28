//! Following a file that is still being written: each full segment is sealed as it arrives, the
//! idle file is closed after a set time, and every segment goes into the home store so the
//! provider can serve it while the file keeps growing.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use ed25519_dalek::SigningKey;
use iroh_blobs::{api::TempTag, store::fs::FsStore};
use sluice_live::{SEGMENT_BYTES, Sender};
use tokio::io::AsyncReadExt;

use super::LiveLog;
use crate::{
    Error, Home,
    engine::{
        Congestion, LiveSend, Outgoing, Provider, RelayModeChoice, now_ms,
        storage::{self},
    },
    ticket::{Body, Ticket},
};

/// How long a file may stay the same size before the stream is closed, unless the sender names
/// another time.
pub const CLOSE_AFTER: Duration = Duration::from_secs(2);

/// Follows `path` and serves it as live content. Only a regular file can be followed: a link could
/// be repointed at another file while the stream is open.
pub async fn prepare_live(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    close_after: Duration,
) -> Result<Outgoing, Error> {
    prepare_live_with_rate(
        home,
        path,
        relay,
        congestion,
        expiry_ms,
        port,
        close_after,
        None,
    )
    .await
}

/// [`prepare_live`] with a byte rate for serving the segments.
#[allow(clippy::too_many_arguments)]
pub async fn prepare_live_with_rate(
    home: &Home,
    path: &Path,
    relay: RelayModeChoice,
    congestion: Congestion,
    expiry_ms: Option<u64>,
    port: Option<u16>,
    close_after: Duration,
    rate: Option<Arc<crate::engine::Rate>>,
) -> Result<Outgoing, Error> {
    let (name, size) = checked_path(path)?;
    let store = FsStore::load(home.path().join("blobs"))
        .await
        .map_err(|e| Error::Storage(format!("cannot open the store: {e}")))?;
    let seed = home.identity()?.secret_seed();
    let key = SigningKey::from_bytes(&seed);
    let sender_key = key.verifying_key().to_bytes();
    let mut stream_id = [0u8; 16];
    getrandom::fill(&mut stream_id)
        .map_err(|e| Error::Storage(format!("cannot name a live stream: {e}")))?;
    let sender = Sender::new(key, stream_id, SEGMENT_BYTES, now_ms())
        .map_err(|e| Error::Storage(format!("cannot start a live stream: {e}")))?;
    let log = Arc::new(LiveLog::new(
        sender.log_open(),
        Arc::new(Mutex::new(std::collections::BTreeSet::new())),
    ));
    let provider = Provider::start_live_with_rate(
        home,
        store.clone(),
        log.clone(),
        relay,
        congestion,
        expiry_ms,
        port,
        rate,
    )
    .await?;
    let ticket = ticket_of(&provider, sender_key, stream_id, expiry_ms)?;
    // The segments stay in the home store while the stream is served, and are given back to the
    // collector when the send stops; the vector is shared with the task that seals them.
    let tags: Arc<Mutex<Vec<TempTag>>> = Arc::default();
    let task = tokio::spawn(follow(
        path.to_path_buf(),
        store,
        log.clone(),
        sender,
        close_after,
        Arc::clone(&tags),
    ));
    Ok(Outgoing {
        provider,
        ticket,
        roots: Vec::new(),
        name: Some(name),
        files: 1,
        bytes: size,
        congestion,
        live: Some(LiveSend::new(log, task, tags)),
        _tags: Vec::new(),
    })
}

/// The name and size of the one regular file a live send follows. A link could be repointed while
/// the stream is open, so it is refused.
fn checked_path(path: &Path) -> Result<(String, u64), Error> {
    let shown = path.display().to_string();
    let meta = fs::symlink_metadata(path).map_err(|e| storage::policy_path("read", path, e))?;
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{shown} is a symlink; follow the file it points at by its real path"
        )));
    }
    if !meta.is_file() {
        return Err(Error::Policy(format!(
            "{shown} is not a regular file, so there is nothing to follow"
        )));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Policy(format!("{shown} has no UTF-8 file name")))?;
    Ok((name.to_string(), meta.len()))
}

fn ticket_of(
    provider: &Provider,
    sender_key: [u8; 32],
    stream_id: [u8; 16],
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
        Body::Live {
            stream_id,
            sender_key,
        },
    )
}

/// Reads the file in segments and pushes them into the log until it stops growing for
/// `close_after`, then signs the close. A full segment is sealed the moment it is read; a short
/// one is the tail and is sealed only when the stream ends.
///
/// A failure ends the stream with the error rather than leaving readers to wait for a close that
/// will never come: the log carries the reason, the readers' streams are reset with it, and the
/// sender stops with a storage error.
async fn follow(
    path: PathBuf,
    store: FsStore,
    log: Arc<LiveLog>,
    sender: Sender,
    close_after: Duration,
    tags: Arc<Mutex<Vec<TempTag>>>,
) {
    if let Err(error) = run(&path, &store, &log, sender, close_after, &tags).await {
        log.fail(&error.to_string());
    }
}

async fn run(
    path: &Path,
    store: &FsStore,
    log: &LiveLog,
    mut sender: Sender,
    close_after: Duration,
    tags: &Mutex<Vec<TempTag>>,
) -> Result<(), Error> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| storage::storage("open", path, e))?;
    // The file that is followed has to stay the same file: a path replaced by another file, or one
    // that shrank, would make the close sign a hash of something that is no longer there.
    let identity = storage::identify(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut pending: Vec<u8> = Vec::with_capacity(SEGMENT_BYTES as usize);
    let mut chunk = vec![0u8; 64 * 1024];
    let mut last_growth = tokio::time::Instant::now();
    let mut read_total = 0u64;
    loop {
        let read = file
            .read(&mut chunk)
            .await
            .map_err(|e| storage::storage("read", path, e))?;
        if read == 0 {
            if let Some(error) = changed(path, identity, read_total)? {
                return Err(error);
            }
            if last_growth.elapsed() >= close_after {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        }
        hasher.update(&chunk[..read]);
        pending.extend_from_slice(&chunk[..read]);
        read_total = read_total.saturating_add(read as u64);
        last_growth = tokio::time::Instant::now();
        while pending.len() >= SEGMENT_BYTES as usize {
            let rest = pending.split_off(SEGMENT_BYTES as usize);
            let segment = std::mem::replace(&mut pending, rest);
            seal(store, log, &mut sender, &segment, tags).await?;
            if let Some(error) = changed(path, identity, read_total)? {
                return Err(error);
            }
        }
    }
    if !pending.is_empty() {
        seal(store, log, &mut sender, &pending, tags).await?;
    }
    let close = sender.close(*hasher.finalize().as_bytes(), now_ms());
    log.finish(close);
    Ok(())
}

/// What it means that the followed file is not the one this stream started on, if it is not. A file
/// that shrank below what was read, or a path that now names a different file, ends the stream: the
/// close would sign bytes that are a consistent file but not the source.
fn changed(
    path: &Path,
    identity: Option<storage::FolderId>,
    read: u64,
) -> Result<Option<Error>, Error> {
    if storage::identify(path)? != identity {
        return Ok(Some(Error::Storage(format!(
            "{} was replaced while it was followed",
            path.display()
        ))));
    }
    let size = fs::metadata(path)
        .map_err(|e| storage::storage("read", path, e))?
        .len();
    if size < read {
        return Ok(Some(Error::Storage(format!(
            "{} shrank from {read} bytes to {size} while it was followed",
            path.display()
        ))));
    }
    Ok(None)
}

/// Keeps one segment in the store under its root and signs the frame that names it.
async fn seal(
    store: &FsStore,
    log: &LiveLog,
    sender: &mut Sender,
    bytes: &[u8],
    tags: &Mutex<Vec<TempTag>>,
) -> Result<(), Error> {
    let tag =
        store.add_slice(bytes).temp_tag().await.map_err(|e| {
            Error::Storage(format!("cannot keep a live segment: {}", crate::plain(&e)))
        })?;
    let root = *tag.hash().as_bytes();
    let frame = sender
        .seal(root, bytes.len() as u32)
        .map_err(|e| Error::Storage(format!("cannot seal a live segment: {e}")))?;
    log.push(frame);
    // Held until the send stops: a reader may ask for any sealed segment until then, so the
    // collector must not free one, and when the send stops all of them go at once.
    tags.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(tag);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CLOSE_AFTER;
    use std::time::Duration;

    #[test]
    fn the_default_close_time_is_two_seconds() {
        assert_eq!(CLOSE_AFTER, Duration::from_secs(2));
    }
}
