//! Fetching every content blob and pack before anything is written, with the queue's retry rule.

use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::plan::{Plan, size_overflow};
use crate::{
    Error,
    engine::{Event, Receiver, emit, fetch},
    manifest::{Kind, Loc, Manifest},
    ticket::Ticket,
};

pub(super) struct Session<'a> {
    pub(super) receiver: &'a Receiver,
    pub(super) ticket: &'a Ticket,
    pub(super) store: &'a FsStore,
    pub(super) manifest: &'a Manifest,
    pub(super) plan: Plan,
    pub(super) events: &'a UnboundedSender<Event>,
    pub(super) rate: Option<&'a crate::engine::Rate>,
    pub(super) max_failures: u32,
}

/// Fetches every content blob and pack before anything is written, retrying a dropped connection
/// with the queue's backoff. A reconnect is a network attempt like any other, so a failure to
/// reconnect is retried too instead of ending a transfer that could still finish. Nothing in the
/// output is touched until the whole tree is in the store, so a retry cannot meet a file it wrote.
/// The connection is handed back so the caller can close it with the completion code.
pub(super) async fn ensure_content(
    session: &Session<'_>,
    mut conn: iroh::endpoint::Connection,
) -> Result<iroh::endpoint::Connection, Error> {
    let mut attempt = 0u32;
    loop {
        let failed = match fetch_content(session, &conn).await {
            Ok(()) => return Ok(conn),
            Err(e) => e,
        };
        if !matches!(failed, Error::Network(_)) || attempt + 1 >= session.max_failures {
            return Err(failed);
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
        match session
            .receiver
            .connect(session.ticket, session.events.clone())
            .await
        {
            Ok(next) => conn = next,
            Err(e) => {
                if !matches!(e, Error::Network(_)) || attempt + 1 >= session.max_failures {
                    return Err(e);
                }
                // The next pass counts this failed attempt and waits again before trying.
            }
        }
    }
}

/// Every pack, then every distinct content blob. A blob that several entries share is fetched
/// once, and its whole size is added to the progress then.
async fn fetch_content(
    session: &Session<'_>,
    conn: &iroh::endpoint::Connection,
) -> Result<(), Error> {
    let shares = shared_bytes(session.manifest)?;
    let mut progress = 0u64;
    for pack in session.manifest.packs() {
        fetch::content_blob(
            session.store,
            conn,
            Hash::from(pack.root),
            pack.size,
            progress,
            session.plan.bytes,
            session.rate,
            session.events,
        )
        .await?;
        progress = progress.saturating_add(pack.size);
        report(session, progress);
    }
    let mut seen = BTreeSet::new();
    for entry in session.manifest.entries() {
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
            session.store,
            conn,
            Hash::from(root),
            entry.size,
            progress,
            session.plan.bytes,
            session.rate,
            session.events,
        )
        .await?;
        progress = progress.saturating_add(shares.get(&root).copied().unwrap_or_default());
        report(session, progress);
    }
    Ok(())
}

/// How many bytes each blob root holds in the manifest, counted once per entry that names it. A
/// hostile manifest can make these add up past `u64::MAX`, which would wrap the progress total.
fn shared_bytes(manifest: &Manifest) -> Result<BTreeMap<[u8; 32], u64>, Error> {
    let mut shares: BTreeMap<[u8; 32], u64> = BTreeMap::new();
    for entry in manifest.entries() {
        if entry.kind != Kind::File {
            continue;
        }
        if let Loc::Blob { root } = entry.loc {
            let total = shares.entry(root).or_default();
            *total = total.checked_add(entry.size).ok_or_else(size_overflow)?;
        }
    }
    Ok(shares)
}

fn report(session: &Session<'_>, progress: u64) {
    emit(
        session.events,
        Event::Progress {
            bytes: progress.min(session.plan.bytes),
            total: session.plan.bytes,
        },
    );
}
