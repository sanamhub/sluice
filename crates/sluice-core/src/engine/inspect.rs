//! What a ticket offers, read from its manifest alone: the names it will write and how much they
//! come to. Nothing is downloaded but the sequence and the manifest, and no store is kept.

use tokio::sync::mpsc::unbounded_channel;

use super::{Congestion, Receiver, RelayModeChoice, Selection, recv::refuse_expired, storage};
use crate::{
    Error,
    manifest::{Kind, Limits, Manifest},
    ticket::{Body, Format, Ticket},
};

/// What a ticket offers: the names a fixed transfer will write, or a live stream, which carries no
/// manifest and so no names and no size.
#[derive(Debug)]
pub enum Offering {
    /// A fixed transfer, whose manifest was read.
    Static(Offered),
    /// A live stream. The ticket names the stream and the sender, and the names and size arrive with
    /// the stream itself.
    Live,
}

/// What a fixed transfer offers.
#[derive(Debug)]
pub struct Offered {
    /// How many files a receive would write.
    pub files: u64,
    /// How many folders.
    pub folders: u64,
    /// How many symlinks.
    pub links: u64,
    /// The total size of the files it would write.
    pub bytes: u64,
    /// How many files the whole manifest holds, which is more than [`Offered::files`] when a
    /// selection takes part of it.
    pub all_files: u64,
    /// The total size of every file in the manifest.
    pub all_bytes: u64,
    /// Whether a selection was given, so the counts are part of a larger transfer.
    pub filtered: bool,
    /// The entries it would write, in the order the manifest lists them.
    pub entries: Vec<OfferedEntry>,
}

/// One name a ticket offers.
#[derive(Debug)]
pub struct OfferedEntry {
    /// The path, with `/` between its components.
    pub path: String,
    /// Which kind of entry it is.
    pub kind: Kind,
    /// The bytes it holds, which is nothing for a folder or a link.
    pub size: u64,
}

impl Offered {
    /// The counts and the names of `manifest`, after the check that its files add up to what the
    /// ticket promises: a size a receive would refuse is not shown as if it would arrive. With a
    /// selection, only the entries it takes are counted and listed, with the folders above them.
    fn of(manifest: &Manifest, ticket: &Ticket, selection: &Selection) -> Result<Self, Error> {
        let all = super::recv::totals(manifest)?;
        super::recv::check_promised(ticket, all.bytes)?;
        let entries = manifest.entries();
        let chosen = super::recv::choose(manifest, selection)?;
        let mut offered = Self {
            files: 0,
            folders: 0,
            links: 0,
            bytes: 0,
            all_files: all.files,
            all_bytes: all.bytes,
            filtered: !selection.is_empty(),
            entries: Vec::with_capacity(chosen.len()),
        };
        for &at in &chosen {
            let entry = &entries[at];
            match entry.kind {
                Kind::File => {
                    offered.files += 1;
                    offered.bytes = offered
                        .bytes
                        .checked_add(entry.size)
                        .ok_or_else(super::recv::size_overflow)?;
                }
                Kind::Dir => offered.folders += 1,
                Kind::Symlink => offered.links += 1,
            }
            offered.entries.push(OfferedEntry {
                path: entry.path.as_str().to_string(),
                kind: entry.kind,
                size: entry.size,
            });
        }
        Ok(offered)
    }
}

/// Reads what a ticket offers, without fetching a byte of content.
///
/// Only the hash sequence and the manifest are fetched, into a throwaway store under the home's
/// private `tmp` that is removed before this returns, under the same limits a receive applies. The
/// output folder is not looked at and nothing is kept in the home. A selection narrows what is
/// described to the entries a receive with that selection would write; `--include` has nothing to
/// choose from on a live or a raw ticket, and is refused there.
pub async fn inspect(
    text: &str,
    limits: Limits,
    include: Option<Selection>,
) -> Result<Offering, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    let selection = include.unwrap_or_default();
    let (size, format) = match ticket.body() {
        // A live stream has no manifest, so there is nothing to fetch and no reason to reach the
        // sender at all.
        Body::Live { .. } => {
            refuse_selection(&selection, "a live ticket has no names to choose from")?;
            return Ok(Offering::Live);
        }
        Body::Static { size, format, .. } => (*size, *format),
    };
    if format == Format::Raw {
        // A raw blob has no names; its size is the whole of what the ticket says.
        refuse_selection(&selection, "a raw ticket has no names to choose from")?;
        return Ok(Offering::Static(Offered {
            files: 1,
            folders: 0,
            links: 0,
            bytes: size,
            all_files: 1,
            all_bytes: size,
            filtered: false,
            entries: Vec::new(),
        }));
    }
    let receiver = Receiver::open(relay_of(&ticket), Congestion::Cubic).await?;
    let result = offer(&receiver, &ticket, limits, &selection).await;
    let closed = receiver.shutdown().await;
    let offering = result?;
    if let Err(e) = closed {
        return Err(Error::Network(format!(
            "the manifest was read but the connection did not close: {e}"
        )));
    }
    Ok(offering)
}

/// Refuses a selection on a ticket that lists no entries.
fn refuse_selection(selection: &Selection, why: &str) -> Result<(), Error> {
    if selection.is_empty() {
        return Ok(());
    }
    Err(Error::Policy(format!(
        "--include takes some of a folder ticket's entries, and {why}"
    )))
}

/// Connects and reads the manifest into a throwaway store of its own, which is removed whether the
/// manifest could be read or not. The manifest is read by the same function a receive uses, so the
/// two cannot disagree about which bytes are the manifest or what it says.
async fn offer(
    receiver: &Receiver,
    ticket: &Ticket,
    limits: Limits,
    selection: &Selection,
) -> Result<Offering, Error> {
    // A channel nobody reads: what a transfer that only reads a manifest reports is not news, and
    // `emit` does nothing once the reader has gone.
    let (events, _unread) = unbounded_channel();
    let conn = receiver.connect(ticket, events.clone()).await?;
    let meta = storage::MetaStore::open().await?;
    let read = super::recv::fetch_manifest(&conn, ticket, meta.store(), limits, &events).await;
    let closed = meta.close().await;
    let manifest = read?;
    closed?;
    Ok(Offering::Static(Offered::of(&manifest, ticket, selection)?))
}

/// The relay the ticket names, if any: a ticket without one is met over a direct path only.
fn relay_of(ticket: &Ticket) -> RelayModeChoice {
    ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    })
}
