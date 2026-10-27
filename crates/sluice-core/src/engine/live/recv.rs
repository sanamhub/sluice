//! Watching a live stream: every segment is fetched as its frame is accepted, appended to a
//! private file, and the close proves the whole file before it takes its name.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use iroh_blobs::{Hash, store::fs::FsStore};
use sluice_live::{Outcome, Receiver as LogReceiver};
use tokio::{io::AsyncWriteExt, sync::mpsc::UnboundedSender};

use super::read_record;
use crate::{
    Error,
    engine::{
        Event, ReceiveOptions, Received, Receiver, RelayModeChoice, emit, fetch, hello, storage,
    },
    ticket::{Body, Ticket},
};

/// Writes the file a live ticket follows to `options.out`, which is the file's path: a live ticket
/// carries no name. Each segment is written as it verifies, and the file is installed only after
/// the close's root and length match what arrived.
pub async fn receive_live(
    text: &str,
    options: &ReceiveOptions,
    store_root: &Path,
    events: &UnboundedSender<Event>,
) -> Result<Received, Error> {
    let ticket = Ticket::decode(text)?;
    let (stream_id, sender_key) = match ticket.body() {
        Body::Live {
            stream_id,
            sender_key,
        } => (*stream_id, *sender_key),
        Body::Static { .. } => {
            return Err(Error::Policy("this ticket is not live".to_string()));
        }
    };
    super::super::recv::refuse_expired(&ticket)?;
    let target = &options.out;
    check_target(target, options.force)?;
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| storage::policy_path("create", parent, e))?;
    let relay = ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    });
    let receiver = Receiver::open(relay, options.congestion).await?;
    let file_tmp = match storage::temp_dir(parent) {
        Ok(path) => Temp(path),
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let store_tmp = match storage::temp_dir(store_root) {
        Ok(path) => Temp(path),
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let store = match FsStore::load(store_tmp.0.join("store")).await {
        Ok(store) => store,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::plain(&e)
            )));
        }
    };
    let started = Instant::now();
    emit(
        events,
        Event::Started {
            name: None,
            files: 1,
            bytes: 0,
            congestion: options.congestion,
        },
    );
    let outcome = exchange(
        &receiver,
        &ticket,
        (stream_id, sender_key),
        &store,
        &file_tmp.0.join("incoming"),
        options,
        events,
    )
    .await;
    let endpoint = receiver.shutdown().await;
    let closed = store.shutdown().await;
    drop(store_tmp);
    let bytes = outcome?;
    drop(file_tmp);
    if endpoint.is_err() || closed.is_err() {
        return Err(Error::Storage(
            "the file is in place but the transfer did not close cleanly".to_string(),
        ));
    }
    emit(events, Event::Verified { bytes });
    emit(
        events,
        Event::Done {
            bytes,
            seconds: started.elapsed().as_secs_f64(),
        },
    );
    Ok(Received {
        path: target.clone(),
        bytes,
        files: 1,
    })
}

/// A live ticket has no name, so the output is the file itself. A folder is refused, and so is a
/// link, which a later write must never go through.
fn check_target(target: &Path, force: bool) -> Result<(), Error> {
    if target.exists() && target.is_dir() {
        return Err(Error::Policy(format!(
            "a live ticket carries no file name, so --out must name the file to write; {} is a \
             folder",
            target.display()
        )));
    }
    let Ok(meta) = fs::symlink_metadata(target) else {
        return Ok(());
    };
    if meta.file_type().is_symlink() {
        return Err(Error::Policy(format!(
            "{} is a link; a live file is never written through one",
            target.display()
        )));
    }
    if !force {
        return Err(storage::clash(target));
    }
    Ok(())
}

async fn exchange(
    receiver: &Receiver,
    ticket: &Ticket,
    stream: ([u8; 16], [u8; 32]),
    store: &FsStore,
    incoming: &Path,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let conn = receiver.connect(ticket, events.clone()).await?;
    let (theirs, _send, mut recv) = hello::greet_stream(&conn)
        .await
        .map_err(|e| Error::Network(format!("the greeting failed: {e}")))?;
    if !theirs.both(&hello::Hello::ours(), hello::CAP_LIVE) {
        return Err(Error::Policy(
            "the provider does not serve live content".to_string(),
        ));
    }
    let open = read_record(&mut recv).await?;
    let mut log = LogReceiver::new(stream.0, stream.1, &open)
        .map_err(|e| Error::Verify(format!("the live stream did not open: {e}")))?;
    let blobs = receiver.connect_static(ticket, events.clone()).await?;
    let mut file = tokio::fs::File::create(incoming)
        .await
        .map_err(|e| storage::storage("create", incoming, e))?;
    let mut hasher = blake3::Hasher::new();
    let mut total = 0u64;
    loop {
        let record = read_record(&mut recv).await?;
        match record.len() {
            sluice_live::Frame::LEN => {
                total = apply_frame(
                    &mut log,
                    &record,
                    store,
                    &blobs,
                    &mut file,
                    &mut hasher,
                    events,
                    total,
                )
                .await?;
            }
            sluice_live::LogClose::LEN => {
                total = close(&mut log, &record, total, &mut hasher)?;
                break;
            }
            _ => {
                return Err(Error::Verify(
                    "a live record is not one of the three the stream carries".to_string(),
                ));
            }
        }
    }
    file.flush()
        .await
        .map_err(|e| storage::storage("write", incoming, e))?;
    let file = file.into_std().await;
    file.sync_all()
        .map_err(|e| storage::storage("sync", incoming, e))?;
    drop(file);
    storage::install(incoming, &options.out, options.force)?;
    Ok(total)
}

/// One frame: fetch its segment and add it to the file. A repeated entry changes nothing; one
/// that breaks the chain or comes out of order stops the transfer.
#[allow(clippy::too_many_arguments)]
async fn apply_frame(
    log: &mut LogReceiver,
    record: &[u8],
    store: &FsStore,
    blobs: &iroh::endpoint::Connection,
    file: &mut tokio::fs::File,
    hasher: &mut blake3::Hasher,
    events: &UnboundedSender<Event>,
    total: u64,
) -> Result<u64, Error> {
    let outcome = log.frame(record);
    let Outcome::Accepted {
        segment_root, len, ..
    } = outcome
    else {
        return match outcome {
            Outcome::Repeat { .. } => Ok(total),
            Outcome::Refused(fault) => Err(Error::Verify(format!(
                "the live chain refused a frame: {fault}"
            ))),
            Outcome::Aborted { seq, reason } => Err(Error::Verify(format!(
                "the live chain was aborted at {seq}: {reason}"
            ))),
            _ => Err(Error::Verify(
                "a live frame did not change the stream".to_string(),
            )),
        };
    };
    let hash = Hash::from(segment_root);
    fetch::content_blob(store, blobs, hash, u64::from(len), 0, 0, events).await?;
    let bytes = fetch::complete_bytes(store, hash).await?.0;
    file.write_all(&bytes)
        .await
        .map_err(|e| storage::storage("write", Path::new("the live file"), e))?;
    hasher.update(&bytes);
    total
        .checked_add(u64::from(len))
        .ok_or_else(|| Error::Verify("the live file is longer than u64::MAX".to_string()))
}

/// The close: the length and the root have to match everything that arrived.
fn close(
    log: &mut LogReceiver,
    record: &[u8],
    total: u64,
    hasher: &mut blake3::Hasher,
) -> Result<u64, Error> {
    match log.close(record) {
        Outcome::Closed {
            final_root,
            total_len,
        } => {
            if total_len != total {
                return Err(Error::Verify(format!(
                    "the close says {total_len} bytes and the segments add up to {total}"
                )));
            }
            if *hasher.finalize().as_bytes() != final_root {
                return Err(Error::Verify(
                    "the file does not hash to the root the close names".to_string(),
                ));
            }
            Ok(total)
        }
        Outcome::Aborted { seq, reason } => Err(Error::Verify(format!(
            "the live chain was aborted at {seq}: {reason}"
        ))),
        other => Err(Error::Verify(format!(
            "the live close was refused: {other:?}"
        ))),
    }
}

/// The file and the store folder, removed however the transfer ends.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        storage::remove_quietly(&self.0);
    }
}
