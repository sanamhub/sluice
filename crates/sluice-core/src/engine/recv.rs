//! A ticket from the receiver's side: prove the sequence and the manifest, check every destination
//! before the first content request, then write the tree into a per-transfer store beside it.

mod content;
mod plan;
mod write;

pub(super) use plan::check_one;
pub(super) use write::write_range;

use std::{fs, path::PathBuf, time::Instant};

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::{Congestion, Event, Receiver, RelayModeChoice, emit, fetch, storage};
use crate::{
    Error,
    manifest::{Kind, Limits, Manifest, parse_sequence},
    ticket::{Body, Format, Ticket},
};

use content::Session;
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
    /// The byte rate this command may use. Every receive of the command shares it.
    pub rate: Option<std::sync::Arc<super::Rate>>,
    /// Fail a write after this many bytes, with the error a full disk gives. A test simulates a
    /// disk that fills mid-transfer; a real caller leaves it as `None`.
    pub write_limit: Option<u64>,
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
    // The ticket's size is the sum of the manifest's files, and a manifest that disagrees with it
    // is refused before any content. Checking the room here means a transfer that cannot fit never
    // opens a store, so its refusal leaves nothing behind.
    if let Body::Static { size, .. } = ticket.body() {
        plan::check_space(options, *size)?;
    }
    let receiver = Receiver::open(relay_of(&ticket), options.congestion).await?;
    let result = run(&receiver, &ticket, root, options, events).await;
    // Closing the endpoint tells the sender that the last bytes arrived, so it can stop without
    // cutting the response off a moment after writing it.
    let closed = receiver.shutdown().await;
    let received = result?;
    if closed.is_err() {
        return Err(Error::Storage(
            "the tree is in place but the transfer did not close cleanly".to_string(),
        ));
    }
    Ok(received)
}

/// One receive, with the partial store beside the output. The manifest is read into a store of its
/// own, because the checks come before this receive may claim a place: a refusal must not leave a
/// store that a later run would resume, and a resume runs every check a first run does.
async fn run(
    receiver: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<Received, Error> {
    let owner = hex(&root);
    let partial = storage::partial_dir(&options.out, &owner);
    let conn = receiver.connect(ticket, events.clone()).await?;
    let manifest = read_manifest(&conn, ticket, options, events).await?;
    let (store, resume, plan) = claim_store(&partial, &owner, &manifest, ticket, options).await?;
    let outcome = exchange(
        receiver, ticket, &manifest, plan, &store, &partial, options, resume, conn, events,
    )
    .await;
    let closed = store.shutdown().await;
    let endpoint = receiver.shutdown().await;
    match outcome {
        Ok((received, seconds)) => {
            if closed.is_err() || endpoint.is_err() {
                return Err(Error::Storage(
                    "the tree is in place but the transfer did not close cleanly".to_string(),
                ));
            }
            // Every content blob was moved or copied out; what is left is metadata.
            storage::remove_quietly(&partial);
            emit(
                events,
                Event::Verified {
                    bytes: received.bytes,
                },
            );
            emit(
                events,
                Event::Done {
                    bytes: received.bytes,
                    seconds,
                },
            );
            Ok(received)
        }
        Err(e) => {
            if !keeps_store(&e) {
                storage::remove_quietly(&partial);
            }
            Err(e)
        }
    }
}

/// Runs every check that needs no content blob, then claims the partial store and opens it. The
/// order is the point: a refusal leaves no store for a later run to resume, and a folder is this
/// transfer's store only when its marker says so. The marker also names the top-level entries this
/// receive made, so a resume may write into those and into nothing else.
async fn claim_store(
    partial: &std::path::Path,
    owner: &str,
    manifest: &Manifest,
    ticket: &Ticket,
    options: &ReceiveOptions,
) -> Result<(FsStore, bool, Plan), Error> {
    let found = storage::recorded(partial)?;
    let resume = found.as_ref().is_some_and(|found| found.root == owner);
    let ours = match found {
        Some(found) if resume => found.names,
        _ => std::collections::BTreeSet::new(),
    };
    let plan = plan::check(manifest, ticket, options, resume, &ours)?;
    if !resume {
        storage::claim(partial)?;
        storage::remember(partial, owner, &top_names(manifest))?;
    }
    match FsStore::load(partial).await {
        Ok(store) => Ok((store, resume, plan)),
        Err(e) => {
            // A store this receive could not open is not one a later one can resume.
            storage::remove_quietly(partial);
            Err(Error::Storage(format!(
                "cannot open the partial store: {}",
                crate::plain(&e)
            )))
        }
    }
}
/// Fetches every content blob and then writes the checked tree.
#[allow(clippy::too_many_arguments)]
async fn exchange(
    receiver: &Receiver,
    ticket: &Ticket,
    manifest: &Manifest,
    plan: Plan,
    store: &FsStore,
    partial: &std::path::Path,
    options: &ReceiveOptions,
    resume: bool,
    conn: iroh::endpoint::Connection,
    events: &UnboundedSender<Event>,
) -> Result<(Received, f64), Error> {
    emit(
        events,
        Event::Started {
            name: single_name(manifest),
            files: plan.files,
            bytes: plan.bytes,
            congestion: options.congestion,
        },
    );
    let started = Instant::now();
    let session = Session {
        receiver,
        ticket,
        store,
        manifest,
        plan,
        events,
        rate: options.rate.as_deref(),
        max_failures: options.max_network_failures,
    };
    let conn = content::ensure_content(&session, conn).await?;
    write::write(manifest, plan, store, options, partial, resume, events).await?;
    // Every file is verified and in place, so the whole tree arrived on this machine. Closing the
    // content connection with the completion code is how the sender learns that, and it is the
    // only moment it may: a receive that failed, or that took a range, never sends it.
    conn.close(super::COMPLETE_CLOSE.into(), b"complete");
    Ok((
        Received {
            path: single_path(manifest, options),
            bytes: plan.bytes,
            files: plan.files,
        },
        started.elapsed().as_secs_f64(),
    ))
}

/// Reads the hash sequence and the manifest into a store of their own, which is removed when this
/// returns. The partial store of the transfer is not touched until every check has passed.
async fn read_manifest(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<Manifest, Error> {
    let meta = storage::MetaStore::open().await?;
    let manifest = fetch_manifest(conn, ticket, &meta.store, options.limits, events).await;
    meta.close().await?;
    manifest
}

/// The top-level names a manifest writes, which are the entries with no `/` in their path.
fn top_names(manifest: &Manifest) -> std::collections::BTreeSet<String> {
    manifest
        .entries()
        .iter()
        .filter(|entry| !entry.path.as_str().contains('/'))
        .map(|entry| entry.path.as_str().to_string())
        .collect()
}

/// Fetches the hash sequence and the manifest and checks that one names the other. The store is
/// open by now because the manifest lives in it; nothing here looks at the output folder.
pub(super) async fn fetch_manifest(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    store: &FsStore,
    limits: crate::manifest::Limits,
    events: &UnboundedSender<Event>,
) -> Result<Manifest, Error> {
    let sequence_root = static_root(ticket)?;
    let sequence_bytes =
        fetch::bounded_blob(store, conn, Hash::from(sequence_root), limits, events).await?;
    let sequence = parse_sequence(&sequence_bytes, limits)?;
    let manifest_root = *sequence
        .first()
        .ok_or_else(|| Error::Policy("the hash sequence names no manifest root".to_string()))?;
    let manifest_bytes =
        fetch::bounded_blob(store, conn, Hash::from(manifest_root), limits, events).await?;
    let manifest = Manifest::decode(&manifest_bytes, limits)?;
    if manifest.hash_sequence() != sequence {
        return Err(Error::Policy(
            "the hash sequence does not match the manifest".to_string(),
        ));
    }
    Ok(manifest)
}

/// The name to put in `started` for a one-file transfer, and none for a tree.
fn single_name(manifest: &Manifest) -> Option<String> {
    single_entry(manifest).map(|entry| entry.path.as_str().to_string())
}

/// The file a one-file transfer wrote, and the output folder for a tree.
fn single_path(manifest: &Manifest, options: &ReceiveOptions) -> PathBuf {
    match single_entry(manifest) {
        Some(entry) => entry.path.under(&options.out),
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

pub(super) fn refuse_expired(ticket: &Ticket) -> Result<(), Error> {
    if ticket.is_expired(super::now_ms()) {
        return Err(Error::Policy(
            "the ticket has expired; ask the sender for a new one".to_string(),
        ));
    }
    Ok(())
}

pub(super) fn relay_of(ticket: &Ticket) -> RelayModeChoice {
    ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    })
}

fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A network failure keeps the store because a second receive continues from it, and so does a
/// storage failure such as a full disk: the bytes already there were verified. A refusal or a bad
/// byte does not, because the store holds nothing a retry could trust.
fn keeps_store(error: &Error) -> bool {
    matches!(error, Error::Network(_) | Error::Storage(_))
}

#[cfg(test)]
mod tests {
    use super::keeps_store;
    use crate::Error;

    #[test]
    fn only_a_network_or_storage_failure_keeps_the_partial_store() {
        assert!(keeps_store(&Error::Network("reset".into())));
        assert!(keeps_store(&Error::Storage("full".into())));
        assert!(!keeps_store(&Error::Verify("mismatch".into())));
        assert!(!keeps_store(&Error::Policy("clash".into())));
    }
}
