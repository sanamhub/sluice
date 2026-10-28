//! Watching a live stream: every segment is fetched as its frame is accepted, appended to a
//! private file, and the close proves the whole file before it takes its name.

use std::{fs, path::Path, time::Instant};

use iroh_blobs::store::fs::FsStore;
use sluice_live::Receiver as LogReceiver;
use tokio::{io::AsyncWriteExt, sync::mpsc::UnboundedSender};

mod limits;
mod records;

use super::read_record;

use crate::{
    Error,
    engine::{Event, ReceiveOptions, Received, Receiver, RelayModeChoice, emit, hello, storage},
    ticket::{Body, Ticket},
};
use limits::Bounds;
use records::records;

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
    let stream = live_ids(&ticket)?;
    super::super::recv::refuse_expired(&ticket)?;
    let opened = open(&ticket, options, store_root).await?;
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
    let outcome = exchange(&opened, &ticket, stream, options, events).await;
    finish(opened, outcome, options, events, started).await
}

/// The stream id and sender key a live ticket names.
fn live_ids(ticket: &Ticket) -> Result<([u8; 16], [u8; 32]), Error> {
    match ticket.body() {
        Body::Live {
            stream_id,
            sender_key,
        } => Ok((*stream_id, *sender_key)),
        Body::Static { .. } => Err(Error::Policy("this ticket is not live".to_string())),
    }
}

/// The socket, the store and the two private folders one live receive holds.
struct Opened {
    receiver: Receiver,
    store: FsStore,
    file_tmp: storage::Temp,
    store_tmp: storage::Temp,
}

/// Opens everything a live receive needs, refusing a target that is a folder or a link first.
async fn open(
    ticket: &Ticket,
    options: &ReceiveOptions,
    store_root: &Path,
) -> Result<Opened, Error> {
    check_target(&options.out, options.force)?;
    let parent = options.out.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| storage::policy_path("create", parent, e))?;
    let relay = ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    });
    let receiver = Receiver::open(relay, options.congestion).await?;
    let file_tmp = match storage::temp_dir(parent) {
        Ok(tmp) => tmp,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let store_tmp = match storage::temp_dir(store_root) {
        Ok(tmp) => tmp,
        Err(e) => {
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    match storage::open_store(&store_tmp.path().join("store"), storage::Collector::On).await {
        Ok(store) => Ok(Opened {
            receiver,
            store,
            file_tmp,
            store_tmp,
        }),
        Err(e) => {
            let _ = receiver.shutdown().await;
            Err(Error::Storage(format!(
                "cannot open a temporary store: {}",
                crate::plain(&e)
            )))
        }
    }
}

/// Closes the socket and the store on every path, removes both folders, and reports the file only
/// when the transfer closed cleanly.
async fn finish(
    opened: Opened,
    outcome: Result<u64, Error>,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
    started: Instant,
) -> Result<Received, Error> {
    let endpoint = opened.receiver.shutdown().await;
    let closed = opened.store.shutdown().await;
    drop(opened.store_tmp);
    let bytes = outcome?;
    drop(opened.file_tmp);
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
        path: options.out.clone(),
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
    opened: &Opened,
    ticket: &Ticket,
    stream: ([u8; 16], [u8; 32]),
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<u64, Error> {
    let incoming = opened.file_tmp.path().join("incoming");
    let conn = opened.receiver.connect(ticket, events.clone()).await?;
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
    let blobs = opened
        .receiver
        .connect_static(ticket, events.clone())
        .await?;
    let mut file = tokio::fs::File::create(&incoming)
        .await
        .map_err(|e| storage::storage("create", &incoming, e))?;
    let mut hasher = blake3::Hasher::new();
    let bounds = Bounds::new(options, opened.store_tmp.path());
    let outcome = records(
        &mut log,
        &mut recv,
        &opened.store,
        &blobs,
        &mut file,
        &mut hasher,
        &bounds,
        options.rate.as_deref(),
        events,
    )
    .await;
    // Whatever arrived is flushed before anything is installed or reported: a stream stopped by the
    // size bound keeps the bytes it wrote, so they have to be on disk first.
    let flushed = flush(&mut file, &incoming).await;
    match outcome {
        Ok(total) => {
            flushed?;
            storage::install(&incoming, &options.out, options.force)?;
            Ok(total)
        }
        Err(stop) => {
            if stop.keep {
                // The bytes that arrived are the start of the file, so they take its name; the
                // temporary store around them is removed either way.
                let _ = flushed;
                let _ = storage::install(&incoming, &options.out, options.force);
            }
            Err(stop.error)
        }
    }
}

/// Flushes and syncs what was written, so an installed prefix is on disk.
async fn flush(file: &mut tokio::fs::File, incoming: &Path) -> Result<(), Error> {
    file.flush()
        .await
        .map_err(|e| storage::storage("write", incoming, e))?;
    file.sync_all()
        .await
        .map_err(|e| storage::storage("sync", incoming, e))
}
