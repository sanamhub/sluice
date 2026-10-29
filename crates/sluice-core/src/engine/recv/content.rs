//! Fetching every content blob and pack before anything is written, with the queue's retry rule.

use std::{collections::BTreeSet, time::Duration};

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::plan::{self, Plan};
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
    pub(super) plan: &'a Plan,
    pub(super) events: &'a UnboundedSender<Event>,
    pub(super) rate: Option<&'a crate::engine::Rate>,
    pub(super) max_failures: u32,
}

/// Fetches every content blob and pack before anything is written, retrying a dropped connection
/// with the queue's backoff. A reconnect is a network attempt like any other, so a failure to
/// reconnect is retried too instead of ending a transfer that could still finish. Nothing in the
/// output is touched until the whole tree is in the store, so a retry cannot meet a file it wrote.
/// The connection is handed back so the caller can close it with the code the run earned.
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

/// Every pack that holds a taken file, then every distinct content blob of a taken file. A blob that
/// several taken entries share is fetched once. A pack is one blob, so a taken file inside it needs
/// the whole pack; a pack with nothing taken is not requested at all. The progress total is the
/// bytes these requests move, which is what the free-space check counted.
async fn fetch_content(
    session: &Session<'_>,
    conn: &iroh::endpoint::Connection,
) -> Result<(), Error> {
    let shares = plan::shared_bytes(session.manifest, &session.plan.chosen)?;
    let mut progress = 0u64;
    for index in plan::packs_of(session.manifest, &session.plan.chosen) {
        let pack = plan::pack(session.manifest, index)?;
        fetch::content_blob(
            session.store,
            conn,
            Hash::from(pack.root),
            pack.size,
            progress,
            session.plan.fetched,
            session.rate,
            session.events,
        )
        .await?;
        progress = progress.saturating_add(pack.size);
        report(session, progress);
    }
    let mut seen = BTreeSet::new();
    for &at in &session.plan.chosen {
        let entry = &session.manifest.entries()[at];
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
            session.plan.fetched,
            session.rate,
            session.events,
        )
        .await?;
        progress = progress.saturating_add(shares.get(&root).copied().unwrap_or_default());
        report(session, progress);
    }
    Ok(())
}

fn report(session: &Session<'_>, progress: u64) {
    emit(
        session.events,
        Event::Progress {
            bytes: progress.min(session.plan.fetched),
            total: session.plan.fetched,
        },
    );
}
