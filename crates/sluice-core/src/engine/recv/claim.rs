//! Claiming the partial store of one transfer, and the metadata store the manifest is read into
//! until every check has passed.

use std::collections::BTreeSet;

use tokio::sync::mpsc::UnboundedSender;

use super::{
    Event, ReceiveOptions, fetch_manifest,
    plan::{self, Plan},
};
use crate::engine::storage;
use crate::{Error, manifest::Manifest, ticket::Ticket};

/// What this home knows about the store folder this run would use.
enum Found {
    /// A store of this home's, in the folder its record names, with a marker that matches the
    /// record: a run that took the same entries may continue it.
    Store(storage::Owner, Option<storage::FolderId>),
    /// The record of a store whose folder is gone. Nothing is left to continue, and the top-level
    /// names that run made are still this ticket's in this output.
    Names(BTreeSet<String>),
    /// No store and no record.
    Nothing,
}

/// Runs every check that needs no content blob, then claims the partial store and opens it. The
/// order is the point: a refusal leaves no store for a later run to resume, and a folder is this
/// transfer's store only when this home has a record of it and its marker carries the record's id.
/// Anything else with that name is refused unread: it could have been planted where the store was,
/// and resuming from it would trust bytes nobody in this home wrote.
pub(super) async fn claim_store(
    partial: &std::path::Path,
    owner: &str,
    manifest: &Manifest,
    ticket: &Ticket,
    options: &ReceiveOptions,
    selection: &crate::engine::Selection,
) -> Result<(storage::Store, bool, Plan), Error> {
    // A store is only continued by a run that takes the same entries: one made for other patterns
    // holds a different set of finished files, and resuming into it would write a tree no run asked
    // for. The store and its record go, and this run claims its own.
    let key = selection.key();
    let (resumed, checked, carried, consume) =
        resumable(decide(partial, owner).await?, partial, &key).await;

    let resume = resumed.is_some();
    check_forced(resumed.as_ref(), options)?;
    // The top-level names a store recorded are this ticket's in this output, whether that store is
    // here to continue or only its record is left: a second selection writes into the folder the
    // first one made instead of needing --force, which would also allow replacing its files.
    let ours = match resumed {
        Some(found) => found.names,
        None => carried,
    };
    let (plan, _created) = plan::check(manifest, ticket, options, resume, &ours, selection)?;
    // Every check passed, so this run is about to claim the folder. A record of names it carried is
    // replaced by its own store's record now; two records for one folder would leave the next run
    // guessing which is its own.
    if consume {
        storage::forget_record(partial);
    }
    // The folder that was checked and the folder that is opened have to be one folder: another user
    // who can write in the output can rename a store away and put one there under the same name
    // between the two calls. A resumed store's identity comes from the handle its marker was read
    // through; a store this run makes is identified as soon as it exists.
    let before = if resume {
        checked
    } else {
        start_store(partial, owner, &plan.ours, options, &key).await?;
        storage::identify(partial)?
    };
    // A store that a cancelled command left sharing an inode with a file in the output is given its
    // own bytes before it is opened: the file is the user's now, and a resumed store must not read
    // bytes that can be changed through it. The folder's identity is checked again inside the
    // repair, before it touches a file, because a folder swapped in since the check above is not
    // this transfer's store.
    storage::repair(partial, before).await?;
    let store = open_claimed(partial, before).await?;
    Ok((store, resume, plan))
}

/// Opens the claimed store and proves it is still the folder that was checked.
async fn open_claimed(
    partial: &std::path::Path,
    before: Option<storage::FolderId>,
) -> Result<storage::Store, Error> {
    let store = match storage::Store::open(partial, storage::Collector::Off).await {
        Ok(store) => store,
        Err(e) => {
            // A store this receive could not open is not one a later one can resume.
            storage::discard_apart(partial).await;
            return Err(Error::Storage(format!(
                "cannot open the partial store: {}",
                crate::plain(&e)
            )));
        }
    };
    if storage::identify(partial)? != before {
        let _ = store.close().await;
        return Err(storage::changed(partial));
    }
    Ok(store)
}

/// What of this home's store there is, what this run may use of it: a store that took the same
/// entries is continued; a store that took others, or only its record is left, gives this run the
/// top-level names it made, and the store itself goes. The last value says whether a record of names
/// must be forgotten once this run claims.
async fn resumable(
    found: Found,
    partial: &std::path::Path,
    key: &str,
) -> (
    Option<storage::Owner>,
    Option<storage::FolderId>,
    BTreeSet<String>,
    bool,
) {
    match found {
        Found::Store(found, _) if found.select.as_deref().unwrap_or("all") != key => {
            let carried = found.names;
            storage::discard_apart(partial).await;
            (None, None, carried, false)
        }
        Found::Store(found, checked) => (Some(found), checked, BTreeSet::new(), false),
        Found::Names(names) => (None, None, names, true),
        Found::Nothing => (None, None, BTreeSet::new(), false),
    }
}

/// A run that was allowed to replace what was in the output was a deliberate choice, so the run that
/// continues it has to make the same one.
fn check_forced(resumed: Option<&storage::Owner>, options: &ReceiveOptions) -> Result<(), Error> {
    if resumed.is_some_and(|found| found.force) && !options.force {
        return Err(Error::Policy(format!(
            "the store this transfer would resume was made with --force; pass --force to continue it into {}",
            options.out.display()
        )));
    }
    Ok(())
}

/// [`resumable_store`] on a blocking thread, with the store's path held.
///
/// The hold is what makes the answer worth anything: a removal another receive started before it
/// was cancelled can still be trying to delete this folder and its record, and that work belongs
/// to a blocking task that outlives the future which asked for it. Taking the hold here means this
/// decision is made about the folder that is there once that removal is over, so a store this run
/// resumes cannot be taken away under it.
async fn decide(partial: &std::path::Path, owner: &str) -> Result<Found, Error> {
    let dir = partial.to_owned();
    let root = owner.to_owned();
    // The task's own failure is wrapped once, with the folder it was about; the storage error the
    // work returns already names that folder and its cause, so it is passed through as it is.
    tokio::task::spawn_blocking(move || storage::with_hold(&dir, || resumable_store(&dir, &root)))
        .await
        .map_err(|error| Error::Storage(format!("cannot read {}: {error}", partial.display())))??
}

/// The marker of the store this transfer may resume and the identity of the folder it was read
/// from, or `None` for both when the folder is not there.
///
/// A folder is this transfer's store only when this home has a record of it, the marker in it is
/// the one that record was written with, and that marker names this ticket's root. Anything else is
/// refused and left alone: it could have been planted where the store was, or be another
/// transfer's, and resuming from it would trust bytes nobody in this home wrote.
fn resumable_store(partial: &std::path::Path, owner: &str) -> Result<Found, Error> {
    let record = storage::find_record(partial, owner)?;
    let exists = std::fs::symlink_metadata(partial).is_ok();
    match (record, exists) {
        (Some(record), true) => {
            let found =
                storage::recorded(partial)?.ok_or_else(|| storage::registry_clash(partial))?;
            let mine = found.owner.id.as_deref();
            if mine != Some(record.id.as_str()) || found.owner.root != owner {
                return Err(storage::registry_clash(partial));
            }
            Ok(Found::Store(found.owner, found.id))
        }
        // A record whose folder is gone. The run that wrote it took part of this ticket into this
        // output, so the top-level names it made are this ticket's here; nothing of its store is
        // left, so nothing is continued.
        (Some(record), false) => Ok(Found::Names(record.names)),
        // A folder with the store's name that this home did not record.
        (None, true) => Err(storage::registry_clash(partial)),
        (None, false) => Ok(Found::Nothing),
    }
}
/// Makes this receive's store: the folder, the home's record of it, and the marker that ties the
/// two together, as one blocking task. A command that is cancelled while this is queued cannot stop
/// it between the folder and the record, so what it leaves is a store the next run resumes.
async fn start_store(
    partial: &std::path::Path,
    owner: &str,
    created: &std::collections::BTreeSet<String>,
    options: &ReceiveOptions,
    selection: &str,
) -> Result<(), Error> {
    let dir = partial.to_owned();
    let root = owner.to_owned();
    let names = created.clone();
    let force = options.force;
    let select = selection.to_owned();
    tokio::task::spawn_blocking(move || storage::begin(&dir, &root, &names, force, &select))
        .await
        .map_err(|error| Error::Storage(format!("cannot claim {}: {error}", partial.display())))?
}

/// Reads the hash sequence and the manifest into a store of their own, which is removed when this
/// returns. The partial store of the transfer is not touched until every check has passed.
pub(super) async fn read_manifest(
    conn: &iroh::endpoint::Connection,
    ticket: &Ticket,
    options: &ReceiveOptions,
    events: &UnboundedSender<Event>,
) -> Result<Manifest, Error> {
    let meta = storage::MetaStore::open().await?;
    let manifest = fetch_manifest(conn, ticket, meta.store(), options.limits, events).await;
    meta.close().await?;
    manifest
}

pub(super) fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::decide;

    /// A folder that cannot be resolved is named once, with its cause, not wrapped in the same
    /// message twice.
    #[tokio::test]
    async fn a_store_decision_that_cannot_resolve_its_folder_says_so_once() {
        let dir = tempfile::tempdir().unwrap();
        let partial = dir.path().join("missing").join(".sluice-partial-x");
        let error = match decide(&partial, "root").await {
            Ok(_) => panic!("a store decision under a folder that is not there succeeded"),
            Err(error) => error.to_string(),
        };
        assert_eq!(error.matches("cannot resolve").count(), 1, "{error}");
        assert_eq!(error.matches("storage error").count(), 1, "{error}");
    }
}
