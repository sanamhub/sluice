//! Claiming the partial store of one transfer, and the metadata store the manifest is read into
//! until every check has passed.

use iroh_blobs::store::fs::FsStore;
use tokio::sync::mpsc::UnboundedSender;

use super::{
    Event, ReceiveOptions, fetch_manifest,
    plan::{self, Plan},
};
use crate::engine::storage;
use crate::{Error, manifest::Manifest, ticket::Ticket};

/// Runs every check that needs no content blob, then claims the partial store and opens it. The
/// order is the point: a refusal leaves no store for a later run to resume, and a folder is this
/// transfer's store only when its marker says so. The marker also names the top-level entries this
/// receive made, so a resume may write into those and into nothing else.
pub(super) async fn claim_store(
    partial: &std::path::Path,
    owner: &str,
    manifest: &Manifest,
    ticket: &Ticket,
    options: &ReceiveOptions,
) -> Result<(FsStore, bool, Plan), Error> {
    let found = storage::recorded(partial)?;
    let resume = found.as_ref().is_some_and(|found| found.root == owner);
    // A run that was allowed to replace what was in the output was a deliberate choice, so the run
    // that continues it has to make the same one.
    if resume && !options.force && found.as_ref().is_some_and(|found| found.force) {
        return Err(Error::Policy(format!(
            "the store this transfer would resume was made with --force; pass --force to continue it into {}",
            options.out.display()
        )));
    }
    let ours = match found {
        Some(found) if resume => found.names,
        _ => std::collections::BTreeSet::new(),
    };
    let (plan, created) = plan::check(manifest, ticket, options, resume, &ours)?;
    if !resume {
        storage::claim(partial)?;
        storage::remember(partial, owner, &created, options.force)?;
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

/// Reads the hash sequence and the manifest into a store of their own, which is removed when this
/// returns. The partial store of the transfer is not touched until every check has passed.
pub(super) async fn read_manifest(
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

pub(super) fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
