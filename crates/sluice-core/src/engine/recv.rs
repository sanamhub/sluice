//! A ticket from the receiver's side: prove the sequence and the manifest, check every destination
//! before the first content request, then write the tree into a per-transfer store beside it.

mod plan;
mod write;

use std::{
    collections::BTreeSet,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::{Congestion, Event, Receiver, RelayModeChoice, emit, fetch, storage};
use crate::{
    Error,
    manifest::{Kind, Limits, Loc, Manifest, parse_sequence},
    ticket::{Body, Format, Ticket},
};

use plan::Plan;

/// Where and how a receive writes.
#[derive(Clone, Debug)]
pub struct ReceiveOptions {
    /// The folder to write into.
    pub out: PathBuf,
    /// Replace a path that is already there instead of refusing.
    pub force: bool,
    /// Do not restore the modification time.
    pub no_times: bool,
    /// Create symlinks the sender recorded. Off unless the user asked.
    pub links: bool,
    /// The manifest and sequence limits to apply.
    pub limits: Limits,
    /// Use this many free bytes instead of asking the volume. A test sets it to simulate a full
    /// disk; a real caller leaves it as `None`.
    pub free_bytes: Option<u64>,
    /// The congestion controller to run.
    pub congestion: Congestion,
}

/// What arrived.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Received {
    /// The file that was written, or the output folder for a tree.
    pub path: PathBuf,
    /// The bytes written.
    pub bytes: u64,
    /// How many files were written.
    pub files: u64,
}

/// Receives what a static ticket names. A folder and a single file take the same path through the
/// manifest, so the checks and the partial store cannot differ between them.
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
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let partial = storage::partial_dir(&options.out, &hex(&root));
    // A store that is already there means an earlier receive of this ticket was interrupted, so a
    // file at a target may be one it already wrote. The writer checks such a file against the
    // content instead of refusing it as a clash.
    let resume = partial.exists();
    fs::create_dir_all(&partial).map_err(|e| storage::storage("create", &partial, e))?;
    let store = FsStore::load(&partial)
        .await
        .map_err(|e| Error::Storage(format!("cannot open the partial store: {e}")))?;

    let result = fetch_all(&receiver, &ticket, &store, options, resume, events).await;
    // Closing the endpoint tells the sender that the last bytes arrived, so it can stop without
    // cutting the response off a moment after writing it.
    let endpoint_closed = receiver.shutdown().await;
    let closed = store.shutdown().await;
    match (result, closed, endpoint_closed) {
        (Ok(received), Ok(()), Ok(())) => {
            // Every content blob was moved or copied out; what is left is metadata.
            let _ = fs::remove_dir_all(&partial);
            Ok(received)
        }
        (Err(e), _, _) => {
            // Deliberately keep the partial store: a second receive continues from it.
            Err(e)
        }
        (Ok(_), Err(_), _) | (Ok(_), _, Err(_)) => Err(Error::Storage(
            "the tree is in place but the transfer did not close cleanly".to_string(),
        )),
    }
}

async fn fetch_all(
    receiver: &Receiver,
    ticket: &Ticket,
    store: &FsStore,
    options: &ReceiveOptions,
    resume: bool,
    events: &UnboundedSender<Event>,
) -> Result<Received, Error> {
    let conn = receiver.connect(ticket, events.clone()).await?;
    let sequence_root = match ticket.body() {
        Body::Static { root, .. } => *root,
        Body::Live { .. } => return Err(Error::Policy("not a static ticket".to_string())),
    };
    let sequence_bytes = fetch::bounded_blob(
        store,
        &conn,
        Hash::from(sequence_root),
        options.limits,
        events,
    )
    .await?;
    let sequence = parse_sequence(&sequence_bytes, options.limits)?;
    let manifest_root = *sequence
        .first()
        .ok_or_else(|| Error::Policy("the hash sequence names no manifest root".to_string()))?;
    let manifest_bytes = fetch::bounded_blob(
        store,
        &conn,
        Hash::from(manifest_root),
        options.limits,
        events,
    )
    .await?;
    let manifest = Manifest::decode(&manifest_bytes, options.limits)?;
    if manifest.hash_sequence() != sequence {
        return Err(Error::Policy(
            "the hash sequence does not match the manifest".to_string(),
        ));
    }
    let plan = plan::check(&manifest, ticket, options, resume)?;
    emit(
        events,
        Event::Started {
            name: single_name(&manifest),
            files: plan.files,
            bytes: plan.bytes,
            congestion: options.congestion,
        },
    );
    let started = Instant::now();
    ensure_content(receiver, ticket, store, &manifest, plan, conn, events).await?;
    write::write(&manifest, plan, store, options, resume).await?;
    emit(events, Event::Verified { bytes: plan.bytes });
    emit(
        events,
        Event::Done {
            bytes: plan.bytes,
            seconds: started.elapsed().as_secs_f64(),
        },
    );
    Ok(Received {
        path: single_path(&manifest, options),
        bytes: plan.bytes,
        files: plan.files,
    })
}

/// Fetches every content blob and pack before anything is written, retrying a dropped connection
/// with the queue's backoff. Nothing in the output is touched until the whole tree is in the
/// store, so a retry cannot meet a file it already wrote.
async fn ensure_content(
    receiver: &Receiver,
    ticket: &Ticket,
    store: &FsStore,
    manifest: &Manifest,
    plan: Plan,
    mut conn: iroh::endpoint::Connection,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let mut attempt = 0u32;
    loop {
        match fetch_content(store, &conn, manifest, plan, events).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = matches!(e, Error::Network(_));
                if !retryable || attempt + 1 >= crate::queue::MAX_NETWORK_FAILURES {
                    return Err(e);
                }
                attempt += 1;
                let wait = crate::queue::backoff_ms(attempt);
                emit(
                    events,
                    Event::Retrying {
                        attempt,
                        seconds: wait / 1000,
                    },
                );
                tokio::time::sleep(Duration::from_millis(wait)).await;
                conn = receiver.connect(ticket, events.clone()).await?;
            }
        }
    }
}

async fn fetch_content(
    store: &FsStore,
    conn: &iroh::endpoint::Connection,
    manifest: &Manifest,
    plan: Plan,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    let mut progress = 0u64;
    for pack in manifest.packs() {
        let hash = Hash::from(pack.root);
        fetch::fetch_blob(store, conn, hash, progress, plan.bytes, events).await?;
        let (_, size) = fetch::complete_bytes(store, hash).await?;
        if size != pack.size {
            return Err(Error::Verify(format!(
                "a pack has {size} bytes and the manifest says {}",
                pack.size
            )));
        }
        progress = progress.saturating_add(pack.size);
        emit(
            events,
            Event::Progress {
                bytes: progress,
                total: plan.bytes,
            },
        );
    }
    let mut seen = BTreeSet::new();
    for entry in manifest.entries() {
        if entry.kind != Kind::File || entry.size == 0 {
            continue;
        }
        let Loc::Blob { root } = entry.loc else {
            continue;
        };
        if !seen.insert(root) {
            continue;
        }
        let hash = Hash::from(root);
        fetch::fetch_blob(store, conn, hash, progress, plan.bytes, events).await?;
        let (_, size) = fetch::complete_bytes(store, hash).await?;
        if size != entry.size {
            return Err(Error::Verify(format!(
                "a blob has {size} bytes and the manifest says {}",
                entry.size
            )));
        }
        let shared: u64 = manifest
            .entries()
            .iter()
            .filter(|other| other.kind == Kind::File && other.loc == Loc::Blob { root })
            .map(|other| other.size)
            .sum();
        progress = progress.saturating_add(shared);
        emit(
            events,
            Event::Progress {
                bytes: progress,
                total: plan.bytes,
            },
        );
    }
    Ok(())
}

/// The name to put in `started` for a one-file transfer, and none for a tree.
fn single_name(manifest: &Manifest) -> Option<String> {
    single_entry(manifest).map(|entry| entry.path.as_str().to_string())
}

/// The file a one-file transfer wrote, and the output folder for a tree.
fn single_path(manifest: &Manifest, options: &ReceiveOptions) -> PathBuf {
    match single_entry(manifest) {
        Some(entry) => options.out.join(entry.path.as_str()),
        None => options.out.clone(),
    }
}

fn single_entry(manifest: &Manifest) -> Option<&crate::manifest::Entry> {
    match manifest.entries() {
        [entry] if entry.kind == Kind::File => Some(entry),
        _ => None,
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
