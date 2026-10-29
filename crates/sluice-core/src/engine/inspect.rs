//! What a ticket offers, read from its manifest alone: the names it will write and how much they
//! come to. Nothing is downloaded but the sequence and the manifest, and no store is kept.

use tokio::sync::mpsc::unbounded_channel;

use super::{Congestion, Receiver, RelayModeChoice, recv::refuse_expired, storage};
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
    /// How many files it writes.
    pub files: u64,
    /// How many folders.
    pub folders: u64,
    /// How many symlinks.
    pub links: u64,
    /// The total size of the files.
    pub bytes: u64,
    /// Every entry, in the order the manifest lists them.
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
    /// ticket promises: a size a receive would refuse is not shown as if it would arrive.
    fn of(manifest: &Manifest, ticket: &Ticket) -> Result<Self, Error> {
        let totals = super::recv::totals(manifest)?;
        super::recv::check_promised(ticket, totals.bytes)?;
        Ok(Self {
            files: totals.files,
            folders: totals.folders,
            links: totals.links,
            bytes: totals.bytes,
            entries: manifest
                .entries()
                .iter()
                .map(|entry| OfferedEntry {
                    path: entry.path.as_str().to_string(),
                    kind: entry.kind,
                    size: entry.size,
                })
                .collect(),
        })
    }
}

/// Reads what a ticket offers, without fetching a byte of content.
///
/// Only the hash sequence and the manifest are fetched, into a throwaway store under the home's
/// private `tmp` that is removed before this returns, under the same limits a receive applies. The
/// output folder is not looked at and nothing is kept in the home.
pub async fn inspect(text: &str, limits: Limits) -> Result<Offering, Error> {
    let ticket = Ticket::decode(text)?;
    refuse_expired(&ticket)?;
    let (size, format) = match ticket.body() {
        // A live stream has no manifest, so there is nothing to fetch and no reason to reach the
        // sender at all.
        Body::Live { .. } => return Ok(Offering::Live),
        Body::Static { size, format, .. } => (*size, *format),
    };
    if format == Format::Raw {
        // A raw blob has no names; its size is the whole of what the ticket says.
        return Ok(Offering::Static(Offered {
            files: 1,
            folders: 0,
            links: 0,
            bytes: size,
            entries: Vec::new(),
        }));
    }
    let receiver = Receiver::open(relay_of(&ticket), Congestion::Cubic).await?;
    let result = offer(&receiver, &ticket, limits).await;
    let closed = receiver.shutdown().await;
    let offering = result?;
    if let Err(e) = closed {
        return Err(Error::Network(format!(
            "the manifest was read but the connection did not close: {e}"
        )));
    }
    Ok(offering)
}

/// Connects and reads the manifest into a throwaway store of its own, which is removed whether the
/// manifest could be read or not. The manifest is read by the same function a receive uses, so the
/// two cannot disagree about which bytes are the manifest or what it says.
async fn offer(receiver: &Receiver, ticket: &Ticket, limits: Limits) -> Result<Offering, Error> {
    // A channel nobody reads: what a transfer that only reads a manifest reports is not news, and
    // `emit` does nothing once the reader has gone.
    let (events, _unread) = unbounded_channel();
    let conn = receiver.connect(ticket, events.clone()).await?;
    let meta = storage::MetaStore::open().await?;
    let read = super::recv::fetch_manifest(&conn, ticket, meta.store(), limits, &events).await;
    let closed = meta.close().await;
    let manifest = read?;
    closed?;
    Ok(Offering::Static(Offered::of(&manifest, ticket)?))
}

/// The relay the ticket names, if any: a ticket without one is met over a direct path only.
fn relay_of(ticket: &Ticket) -> RelayModeChoice {
    ticket.relay_url().map_or(RelayModeChoice::Disabled, |url| {
        RelayModeChoice::Url(url.to_string())
    })
}
