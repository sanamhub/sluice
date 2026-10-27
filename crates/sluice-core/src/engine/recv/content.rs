//! Fetching every content blob and pack before anything is written, with the queue's retry rule.

use std::{collections::BTreeSet, time::Duration};

use iroh_blobs::{Hash, store::fs::FsStore};
use tokio::sync::mpsc::UnboundedSender;

use super::plan::Plan;
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
    pub(super) max_failures: u32,
}

/// Fetches every content blob and pack before anything is written, retrying a dropped connection
/// with the queue's backoff. Nothing in the output is touched until the whole tree is in the
/// store, so a retry cannot meet a file it already wrote.
pub(super) async fn ensure_content(
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
