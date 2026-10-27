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

/// The top-level rules of a receive, as a decision that touches no file system: whether `name` (a
/// path from a manifest) is allowed to be written at the top of the output folder. `Err("hidden")`
/// is a hidden name the user did not allow, and `Err("clash")` a name already there.
///
/// # Examples
///
/// ```
/// use sluice_core::engine::top_level_rule;
///
/// assert!(top_level_rule("proj", false, false, false, false).is_ok());
/// assert_eq!(top_level_rule(".ssh", false, false, false, false), Err("hidden"));
/// assert_eq!(top_level_rule("proj", true, false, false, false), Err("clash"));
/// assert!(top_level_rule("proj", true, false, true, false).is_ok());
/// assert!(top_level_rule("proj/a", true, false, false, false).is_ok());
/// ```
pub fn top_level_rule(
    name: &str,
    exists: bool,
    allow_hidden: bool,
    replace: bool,
    resume: bool,
) -> Result<(), &'static str> {
    plan::top_level(name, exists, allow_hidden, replace, resume)
}

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
    /// Accept a top-level name that starts with a dot. Off unless the user asked.
    pub allow_hidden: bool,
    /// The manifest and sequence limits to apply.
    pub limits: Limits,
    /// Use this many free bytes instead of asking the volume. A test sets it to simulate a full
    /// disk; a real caller leaves it as `None`.
    pub free_bytes: Option<u64>,
    /// The congestion controller to run.
    pub congestion: Congestion,
    /// How many network failures in a row a receive retries before it gives up. A real caller uses
    /// [crate::queue::MAX_NETWORK_FAILURES]; a test lowers it to reach the end quickly.
    pub max_network_failures: u32,
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
    let root = static_root(&ticket)?;
    fs::create_dir_all(&options.out)
        .map_err(|e| storage::policy_path("create", &options.out, e))?;
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let partial = storage::partial_dir(&options.out, &hex(&root));
    // A store that is already there means an earlier receive of this ticket was interrupted, so a
    // file at a target may be one it already wrote. The writer checks such a file against the
    // content instead of refusing it as a clash.
    let resume = partial.exists();
    fs::create_dir_all(&partial).map_err(|e| storage::storage("create", &partial, e))?;
    let store = match FsStore::load(&partial).await {
        Ok(store) => store,
        Err(e) => {
            // A store this receive could not open is not one a later one can resume.
            if !resume {
                storage::remove_quietly(&partial);
            }
            let _ = receiver.shutdown().await;
            return Err(Error::Storage(format!(
                "cannot open the partial store: {}",
                crate::chain(&e)
            )));
        }
    };
    let conn = match receiver.connect(&ticket, events.clone()).await {
        Ok(conn) => conn,
        Err(e) => {
            let _ = store.shutdown().await;
            if !resume {
                storage::remove_quietly(&partial);
            }
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
    let prepared = prepare(&conn, &ticket, &store, options, resume, events).await;
    let (manifest, plan) = match prepared {
        Ok(pair) => pair,
        Err(e) => {
            // Nothing was written, so a store made by this run is only a refusal's leftovers. One
            // that was already there holds content a second receive can continue from.
            let _ = store.shutdown().await;
            if !resume {
                storage::remove_quietly(&partial);
            }
            let _ = receiver.shutdown().await;
            return Err(e);
        }
    };
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
    let session = Session {
        receiver: &receiver,
        ticket: &ticket,
        store: &store,
        manifest: &manifest,
        plan,
        events,
        max_failures: options.max_network_failures,
    };
    let outcome = ensure_content(&session, conn).await;
    // Closing the endpoint tells the sender that the last bytes arrived, so it can stop without
    // cutting the response off a moment after writing it.
    let endpoint_closed = receiver.shutdown().await;
    if let Err(e) = outcome {
        let _ = store.shutdown().await;
        if !keeps_store(&e) {
            storage::remove_quietly(&partial);
        }
        return Err(e);
    }
    let written = write::write(&manifest, plan, &store, options, resume, events).await;
    let closed = store.shutdown().await;
    written?;
    if closed.is_err() || endpoint_closed.is_err() {
        return Err(Error::Storage(
            "the tree is in place but the transfer did not close cleanly".to_string(),
        ));
    }
    // Every content blob was moved or copied out; what is left is metadata.
    storage::remove_quietly(&partial);
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
/// Fetches the sequence and the manifest and runs every check that needs no content blob. The
/// store is open by now because the manifest lives in it.
async fn prepare(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    options: &ReceiveOptions,
    resume: bool,
    events: &UnboundedSender<Event>,
) -> Result<(Manifest, Plan), Error> {
    let sequence_root = static_root(ticket)?;
    let sequence_bytes = fetch::bounded_blob(
        store,
        conn,
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
        conn,
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
    Ok((manifest, plan))
}

/// What a receive fetches and writes, bundled so the retry loop takes one argument.
struct Session<'a> {
    receiver: &'a Receiver,
    ticket: &'a Ticket,
    store: &'a FsStore,
    manifest: &'a Manifest,
    plan: Plan,
    events: &'a UnboundedSender<Event>,
    max_failures: u32,
}

/// Fetches every content blob and pack before anything is written, retrying a dropped connection
/// with the queue's backoff. Nothing in the output is touched until the whole tree is in the
/// store, so a retry cannot meet a file it already wrote.
async fn ensure_content(
    session: &Session<'_>,
    mut conn: iroh::endpoint::Connection,
) -> Result<(), Error> {
    let mut attempt = 0u32;
    loop {
        match fetch_content(session, &conn).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                let retryable = matches!(e, Error::Network(_));
                if !retryable || attempt + 1 >= session.max_failures {
                    return Err(e);
                }
                attempt += 1;
                let wait = crate::queue::backoff_ms(attempt);
                emit(
                    session.events,
                    Event::Retrying {
                        attempt,
                        seconds: wait / 1000,
                    },
                );
                tokio::time::sleep(Duration::from_millis(wait)).await;
                conn = session
                    .receiver
                    .connect(session.ticket, session.events.clone())
                    .await?;
            }
        }
    }
}

async fn fetch_content(
    session: &Session<'_>,
    conn: &iroh::endpoint::Connection,
) -> Result<(), Error> {
    let store = session.store;
    let manifest = session.manifest;
    let plan = session.plan;
    let events = session.events;
    let mut progress = 0u64;
    for pack in manifest.packs() {
        fetch::content_blob(
            store,
            conn,
            Hash::from(pack.root),
            pack.size,
            progress,
            plan.bytes,
            events,
        )
        .await?;
        progress = progress.saturating_add(pack.size);
        emit(
            events,
            Event::Progress {
                bytes: progress.min(plan.bytes),
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
        fetch::content_blob(
            store,
            conn,
            Hash::from(root),
            entry.size,
            progress,
            plan.bytes,
            events,
        )
        .await?;
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
                bytes: progress.min(plan.bytes),
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

fn static_root(ticket: &Ticket) -> Result<[u8; 32], Error> {
    match ticket.body() {
        Body::Static {
            root,
            format: Format::HashSequence,
            ..
        } => Ok(*root),
        Body::Static { .. } => Err(Error::Policy(
            "this ticket is a raw stream, not supported yet".to_string(),
        )),
        Body::Live { .. } => Err(Error::Policy(
            "this is a live ticket, not supported yet".to_string(),
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

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A network failure keeps the store because a second receive continues from it. A refusal or a
/// bad byte does not: the store holds nothing a retry could trust.
fn keeps_store(error: &Error) -> bool {
    matches!(error, Error::Network(_))
}

#[cfg(test)]
mod tests {
    use super::keeps_store;
    use crate::Error;

    #[test]
    fn only_a_network_failure_keeps_the_partial_store() {
        assert!(keeps_store(&Error::Network("reset".into())));
        assert!(!keeps_store(&Error::Verify("mismatch".into())));
        assert!(!keeps_store(&Error::Policy("clash".into())));
        assert!(!keeps_store(&Error::Storage("full".into())));
    }
}
