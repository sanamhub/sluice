//! The sender's half of a delivery: reading one request off a `sluice/1` stream, answering it, and
//! counting the answer once the asker has it.

use std::time::Duration;

use iroh::endpoint::{RecvStream, SendStream};
use n0_future::time::timeout;
use tokio::io::AsyncWriteExt;

#[cfg(test)]
use super::lock;
use super::{HOLD, Receipt, Receipts};
use crate::engine::hello::{read_stream_varint, take_varint, varint_bytes};
use crate::engine::now_ms;

/// The bytes of a request: one root.
pub(super) const REQUEST_LEN: u64 = 32;
/// How long a sender waits for the request after the greeting, so a peer that greets and says
/// nothing cannot hold the connection open.
const REQUEST_WAIT: Duration = Duration::from_secs(10);

/// Answers one request on a `sluice/1` stream, after both greetings, and finishes the stream. A
/// request that is not 32 bytes or arrives too late writes nothing, and so does a refused one. The
/// answer is counted once the asker has it, or has closed the connection, and never later than
/// [`HOLD`] after it was written.
pub(in crate::engine) async fn serve_request(
    receipts: &Receipts,
    conn: &iroh::endpoint::Connection,
    recv: &mut RecvStream,
    mut send: SendStream,
) -> std::io::Result<()> {
    let peer = *conn.remote_id().as_bytes();
    let Ok(Ok(root)) = timeout(REQUEST_WAIT, read_request(recv)).await else {
        return send.finish().map_err(std::io::Error::other);
    };
    let Some(receipt) = receipts.answer(peer, root, now_ms()) else {
        return send.finish().map_err(std::io::Error::other);
    };
    receipts.begin();
    let written = write_answer(receipts, &receipt, &mut send).await;
    let delivered = match written {
        Ok(true) if send.finish().is_ok() => taken(&send, conn).await,
        _ => false,
    };
    receipts.settle(delivered);
    Ok(())
}

/// Writes the record, and says whether one was written.
async fn write_answer(
    receipts: &Receipts,
    receipt: &Receipt,
    send: &mut SendStream,
) -> std::io::Result<bool> {
    #[cfg(test)]
    {
        let delay = *lock(&receipts.delay);
        tokio::time::sleep(delay).await;
        let forced = lock(&receipts.forced).clone();
        if let Some(record) = forced {
            if record.is_empty() {
                // A sender that writes nothing: hold the stream until the dialer gives up, so the
                // receiver's wait is what ends the exchange.
                let _ = send.stopped().await;
                return Ok(false);
            }
            send.write_all(&record).await?;
            send.flush().await?;
            return Ok(true);
        }
    }
    #[cfg(not(test))]
    let _ = receipts;
    let mut record = varint_bytes(crate::receipt::RECORD_LEN as u64);
    record.extend_from_slice(&receipt.to_bytes());
    send.write_all(&record).await?;
    send.flush().await?;
    Ok(true)
}

/// Waits for the asker to take what was written: the send stream is acknowledged or stopped, or the
/// asker closes the connection, and never longer than [`HOLD`]. Returns whether it did, so a record
/// that may still be in flight is not counted as delivered.
async fn taken(send: &SendStream, conn: &iroh::endpoint::Connection) -> bool {
    tokio::select! {
        _ = send.stopped() => true,
        _ = conn.closed() => true,
        () = tokio::time::sleep(HOLD) => false,
    }
}

/// The root a request stream holds, as `SPEC.md` section 10 spells it: a length that is 32 in its
/// shortest varint, the 32 bytes of the root, and nothing after them. `stream` is everything the
/// asker wrote before it ended its side. Any other stream is refused.
///
/// # Errors
///
/// `Error::Policy` when the stream is not exactly one request.
///
/// # Examples
///
/// ```
/// use sluice_core::engine::parse_request;
///
/// let mut stream = vec![32];
/// stream.extend_from_slice(&[7; 32]);
/// assert_eq!(parse_request(&stream).unwrap(), [7; 32]);
/// stream.push(0);
/// assert!(parse_request(&stream).is_err());
/// ```
pub fn parse_request(stream: &[u8]) -> Result<[u8; 32], crate::Error> {
    let mut at = 0;
    if take_varint(stream, &mut at) != Some(REQUEST_LEN) {
        return Err(refuse("a receipt request is 32 bytes"));
    }
    root_after_length(&stream[at..])
}

/// The root in what follows a request's length, which is the whole root and nothing more.
fn root_after_length(rest: &[u8]) -> Result<[u8; 32], crate::Error> {
    rest.try_into()
        .map_err(|_| refuse("a receipt request is the length, the root and the end of the stream"))
}

fn refuse(reason: &str) -> crate::Error {
    crate::Error::Policy(reason.to_string())
}

/// One request: its length as a varint, which must be 32, then the root, then the end of the
/// stream.
async fn read_request(recv: &mut RecvStream) -> std::io::Result<[u8; 32]> {
    let len = read_stream_varint(recv).await?;
    if len != REQUEST_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "a receipt request is 32 bytes",
        ));
    }
    // The request is the whole of what the asker sends, so its end is part of it: one byte more
    // than the root is read, and a stream that goes on or never ends is answered with nothing.
    let rest = recv
        .read_to_end(REQUEST_LEN as usize + 1)
        .await
        .map_err(std::io::Error::other)?;
    root_after_length(&rest).map_err(std::io::Error::other)
}
