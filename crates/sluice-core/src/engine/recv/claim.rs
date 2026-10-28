//! Claiming the partial store of one transfer, and the metadata store the manifest is read into
//! until every check has passed.

use tokio::sync::mpsc::UnboundedSender;

use super::{
    Event, ReceiveOptions, fetch_manifest,
    plan::{self, Plan},
};
use crate::engine::storage;
use crate::{Error, manifest::Manifest, ticket::Ticket};

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
) -> Result<(storage::Store, bool, Plan), Error> {
    let (resumed, checked) = resumable_store(partial, owner)?;
    let resume = resumed.is_some();
    // A run that was allowed to replace what was in the output was a deliberate choice, so the run
    // that continues it has to make the same one.
    if resume && !options.force && resumed.as_ref().is_some_and(|found| found.force) {
        return Err(Error::Policy(format!(
            "the store this transfer would resume was made with --force; pass --force to continue it into {}",
            options.out.display()
        )));
    }
    let ours = resumed.map(|found| found.names).unwrap_or_default();
    let (plan, created) = plan::check(manifest, ticket, options, resume, &ours)?;
    // The folder that was checked and the folder that is opened have to be one folder: another user
    // who can write in the output can rename a store away and put one there under the same name
    // between the two calls. A resumed store's identity comes from the handle its marker was read
    // through; a store this run makes is identified as soon as it exists.
    let before = if resume {
        checked
    } else {
        start_store(partial, owner, &created, options).await?;
        storage::identify(partial)?
    };
    // A store that a cancelled command left sharing an inode with a file in the output is given its
    // own bytes before it is opened: the file is the user's now, and a resumed store must not read
    // bytes that can be changed through it. The folder's identity is checked again inside the
    // repair, before it touches a file, because a folder swapped in since the check above is not
    // this transfer's store.
    storage::repair(partial, before).await?;
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
    Ok((store, resume, plan))
}

/// The marker of the store this transfer may resume and the identity of the folder it was read
/// from, or `None` for both when the folder is not there.
///
/// A folder is this transfer's store only when this home has a record of it, the marker in it is
/// the one that record was written with, and that marker names this ticket's root. Anything else is
/// refused and left alone: it could have been planted where the store was, or be another
/// transfer's, and resuming from it would trust bytes nobody in this home wrote.
fn resumable_store(
    partial: &std::path::Path,
    owner: &str,
) -> Result<(Option<storage::Owner>, Option<storage::FolderId>), Error> {
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
            Ok((Some(found.owner), found.id))
        }
        (Some(_), false) => {
            // A record of a store that is gone: this home left it behind, so drop it.
            storage::forget_record(partial);
            Ok((None, None))
        }
        // A folder with the store's name that this home did not record.
        (None, true) => Err(storage::registry_clash(partial)),
        (None, false) => Ok((None, None)),
    }
}
/// Makes this receive's store: the folder, the home's record of it, and the marker that ties the
/// two together. A marker that cannot be written leaves no store and no record, so the next run
/// starts from an empty folder rather than from one whose marker says something else.
async fn start_store(
    partial: &std::path::Path,
    owner: &str,
    created: &std::collections::BTreeSet<String>,
    options: &ReceiveOptions,
) -> Result<(), Error> {
    // Claiming removes a folder left under the store's name, which can wait for a held file; the
    // waits are taken on a blocking thread so a receive does not hold its runtime while they run.
    let dir = partial.to_owned();
    tokio::task::spawn_blocking(move || storage::claim(&dir))
        .await
        .map_err(|error| {
            Error::Storage(format!("cannot claim {}: {error}", partial.display()))
        })??;
    let id = storage::remember_record(partial, owner)?;
    if let Err(e) = storage::remember(partial, owner, created, options.force, &id) {
        storage::discard_apart(partial).await;
        return Err(e);
    }
    Ok(())
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
