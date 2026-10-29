//! The end of a receive: what it says it is about to write, the code its content connection earns,
//! and what it reports once the files are in place.

use tokio::sync::mpsc::UnboundedSender;

use super::{Event, ReceiveOptions, Received, Selection, emit, plan, single_name, storage};
use crate::Error;

/// The end of a receive that arrived: a store or endpoint that would not close is a warning, and the
/// store is removed, keeping its record of names when this run took part of the ticket.
pub(super) async fn finished(
    received: Received,
    seconds: f64,
    closed: Result<(), Error>,
    endpoint: Result<(), Error>,
    events: &UnboundedSender<Event>,
    selection: &Selection,
    partial: &std::path::Path,
) -> Received {
    // A completion code, when this run took every entry, went out before this point, so the files
    // are in place and the sender has been told so. A shutdown failure is not a reason to report a
    // transfer that arrived as failed, or to keep a store a later run would try to resume into a
    // tree that is already there.
    if let Some(message) = did_not_close(closed, endpoint) {
        emit(events, Event::Warning { message });
    }
    // Every content blob was moved or copied out; what is left is metadata. A run that took part of
    // the ticket keeps the record of the top-level names it made, so a later selection of the same
    // ticket into this output writes beside them without --force; a run that took the whole ticket
    // leaves nothing at all.
    if selection.is_empty() {
        storage::discard_apart(partial).await;
    } else {
        storage::discard_apart_keeping_names(partial).await;
    }
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
    received
}

/// Says what the run is about to write, and what part of the ticket that is when a selection took
/// only some of it.
pub(super) fn announce(
    manifest: &crate::manifest::Manifest,
    plan: &plan::Plan,
    options: &ReceiveOptions,
    selection: &Selection,
    events: &UnboundedSender<Event>,
) -> Result<(), Error> {
    // What the whole manifest holds: the planned line says what is coming out of what the ticket
    // offers, so a user knows the selection did something.
    let of = if selection.is_empty() {
        None
    } else {
        Some(plan::totals(manifest)?)
    };
    emit(
        events,
        Event::Planned {
            files: plan.files,
            folders: plan.folders,
            links: plan.links,
            bytes: plan.bytes,
            of_files: of.as_ref().map(|totals| totals.files),
            of_bytes: of.as_ref().map(|totals| totals.bytes),
            // The line counts what lands on disk, and a pack is fetched whole: when that moves more
            // bytes than the entries it writes, the line says how much.
            fetched: (plan.fetched != plan.bytes).then_some(plan.fetched),
        },
    );
    emit(
        events,
        Event::Started {
            name: single_name(manifest),
            files: plan.files,
            bytes: plan.bytes,
            congestion: options.congestion,
        },
    );
    Ok(())
}

/// Ends the content connection with the code the run earned. The completion code says the whole
/// transfer arrived, so it goes out only when every entry of the manifest was taken: the sender
/// believes it only from a connection that asked for every root, and a run that took part of a tree
/// must not claim a download it did not finish. Such a run closes the ordinary way, so a one-time
/// send keeps serving whoever wants all of it.
pub(super) fn close_content(conn: iroh::endpoint::Connection, whole: bool) {
    if whole {
        conn.close(crate::engine::COMPLETE_CLOSE.into(), b"complete");
    } else {
        conn.close(0u32.into(), b"");
    }
}

/// The warning a finished receive prints when a store or the endpoint would not close. A shutdown
/// failure after the completion code does not change what is on disk, so it is one line and not a
/// failed transfer.
pub(super) fn did_not_close(
    store: Result<(), impl std::fmt::Display>,
    endpoint: Result<(), impl std::fmt::Display>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Err(e) = store {
        parts.push(format!("the partial store did not close: {e}"));
    }
    if let Err(e) = endpoint {
        parts.push(format!("the connection did not close: {e}"));
    }
    if parts.is_empty() {
        return None;
    }
    Some(format!(
        "the tree is in place, but {}; the files are unaffected",
        parts.join(", and ")
    ))
}

/// A network failure keeps the store because a second receive continues from it, and so does a
/// storage failure such as a full disk: the bytes already there were verified. A refusal or a bad
/// byte does not, because the store holds nothing a retry could trust.
pub(super) fn keeps_store(error: &Error) -> bool {
    matches!(error, Error::Network(_) | Error::Storage(_))
}

#[cfg(test)]
mod tests {
    use super::{did_not_close, keeps_store};
    use crate::Error;

    #[test]
    fn only_a_network_or_storage_failure_keeps_the_partial_store() {
        assert!(keeps_store(&Error::Network("reset".into())));
        assert!(keeps_store(&Error::Storage("full".into())));
        assert!(!keeps_store(&Error::Verify("mismatch".into())));
        assert!(!keeps_store(&Error::Policy("clash".into())));
    }

    /// Once the completion code is sent the files are in place, so a shutdown that failed is a
    /// warning. Reporting it as an error left the store behind for a resume the tree no longer
    /// needs, and told the user a transfer that arrived had failed.
    #[test]
    fn a_shutdown_failure_after_the_code_is_a_warning() {
        assert!(did_not_close(Ok::<(), Error>(()), Ok::<(), Error>(())).is_none());
        let message =
            did_not_close(Err(Error::Storage("the store".into())), Ok::<(), Error>(())).unwrap();
        assert!(message.contains("in place") && message.contains("the store"));
        let both = did_not_close(
            Err(Error::Storage("the store".into())),
            Err(Error::Network("the endpoint".into())),
        )
        .unwrap();
        assert!(both.contains("the store") && both.contains("the endpoint"));
    }
}
