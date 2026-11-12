//! The requester's half: the root it asks about, the signature list that comes back, and the one
//! message it sends after it, `NeedRanges`.
//!
//! Nothing derived from the candidate leaves this side except the ranges it still needs. What that
//! tells the sender is written in `SPEC.md` section 7 and in ADR 0035.

use iroh::endpoint::Connection;
use n0_future::time::timeout;
use sluice_delta::{NeedRanges, SigList};
use tokio::sync::mpsc::UnboundedSender;

use super::{MAX_SIG_FRAME, frame, read_frame};
use crate::engine::hello::{CAP_DELTA, Hello, MAX_FRAME_BYTES, greet_stream_with};
use crate::engine::{ALPN, Event, Receiver};
use crate::ticket::Ticket;

/// The connection of one file's control exchange, kept open until the ranges are fetched.
pub(in crate::engine) struct Control {
    conn: Connection,
}

/// Asks the sender of `ticket` for the signatures of the content with `root`, which is `size` bytes.
/// `None` is every way it can fail: no connection, a sender that does not set `DELTA`, one that
/// declines, a frame that breaks a rule of section 7, a list for content of another length. The
/// caller fetches plainly in all of them. The second value is how many bytes the list took.
pub(in crate::engine) async fn signatures(
    receiver: &Receiver,
    ticket: &Ticket,
    root: [u8; 32],
    size: u64,
    wait: std::time::Duration,
    events: &UnboundedSender<Event>,
) -> Option<(Control, SigList, u64)> {
    let conn = receiver
        .connect_alpn(ticket, ALPN, events.clone())
        .await
        .ok()?;
    let asked = timeout(wait, ask(&conn, root, size)).await;
    match asked {
        Ok(Some((list, bytes))) => Some((Control { conn }, list, bytes)),
        _ => {
            conn.close(0u32.into(), b"");
            None
        }
    }
}

/// The exchange on one connection: greet, send the root, read one frame, decode it, check it.
async fn ask(conn: &Connection, root: [u8; 32], size: u64) -> Option<(SigList, u64)> {
    let ours = Hello::ours().with(CAP_DELTA);
    let (theirs, mut send, mut recv) = greet_stream_with(conn, ours).await.ok()?;
    if !ours.both(&theirs, CAP_DELTA) {
        return None;
    }
    send.write_all(&root).await.ok()?;
    send.finish().ok()?;
    let limit = MAX_SIG_FRAME.min(u64::from(MAX_FRAME_BYTES));
    let bytes = read_frame(&mut recv, limit).await.ok()?;
    let list = SigList::decode(&bytes).ok()?;
    // A list whose lengths are not the length of the content was cut from other content.
    (list.total_len() == size).then_some((list, bytes.len() as u64))
}

impl Control {
    /// Sends the ranges the requester still needs, in a stream of its own, and ends it. A failure
    /// is not one: the sender does nothing with the ranges, and the fetch does not depend on them.
    pub(in crate::engine) async fn need(&self, need: &NeedRanges) {
        let Ok(mut send) = self.conn.open_uni().await else {
            return;
        };
        let sent = send.write_all(&frame(&need.encode())).await;
        if sent.is_ok() && send.finish().is_ok() {
            // The close that follows would discard what is still in flight.
            let _ = timeout(super::REQUEST_WAIT, send.stopped()).await;
        }
    }

    /// Ends the exchange. The sender has what it was sent, and nothing more is read from it.
    pub(in crate::engine) fn close(self) {
        self.conn.close(0u32.into(), b"");
    }
}
