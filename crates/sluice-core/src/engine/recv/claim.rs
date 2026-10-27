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

/// The top-level names a manifest writes, which are the entries with no `/` in their path.
fn top_names(manifest: &Manifest) -> std::collections::BTreeSet<String> {
    manifest
        .entries()
        .iter()
        .filter(|entry| !entry.path.as_str().contains('/'))
        .map(|entry| entry.path.as_str().to_string())
        .collect()
}

pub(super) fn hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
