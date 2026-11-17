//! The device side of pairing: redeeming a ticket, and handing a delivery to a daemon. Neither
//! stores a ticket; the daemon's identity is what a grant names, and it presents it again.

use iroh::Endpoint;

use super::super::{ALPN_DELIVER, ALPN_PAIR, Congestion, Receiver, RelayModeChoice};
use super::{Body, Error, Home, Ticket};
use crate::device::Grant;
use crate::paired::PairedDaemon;
/// What a pairing stream answered a device that redeemed a ticket.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairOutcome {
    /// `0x00`: the daemon wrote the grant.
    Accepted(Grant),
    /// `0x01`: the ticket did not match, expired, belonged to another device, or was revoked.
    Refused,
    /// `0x02`: the daemon could not read or write its grants file.
    Unreadable,
}

/// Redeems a pairing ticket from this device. The device binds an endpoint with the home's identity,
/// opens a pairing stream on the daemon's endpoint, writes the ticket's secret and answers the status
/// byte. The same identity presents the grant later, so the ticket is not stored: only its endpoint
/// id, addresses and grant belong in a caller's list.
pub async fn redeem(home: &Home, text: &str) -> Result<PairOutcome, Error> {
    let ticket = Ticket::decode(text)?;
    let (secret, grant) = match ticket.body() {
        Body::Pairing { secret, grant } => (*secret, *grant),
        _ => {
            return Err(Error::Policy(
                "this is not a pairing ticket; redeem a ticket `sluice pair` printed".to_string(),
            ));
        }
    };
    let relay = match ticket.relay_url() {
        Some(url) => RelayModeChoice::Url(url.to_string()),
        None => RelayModeChoice::Disabled,
    };
    let seed = home.identity()?.secret_seed();
    let receiver = Receiver::open_with(Some(seed), relay, Congestion::Cubic).await?;
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let conn = receiver.connect_alpn(&ticket, ALPN_PAIR, events).await?;
    let answer = async {
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| Error::Network(format!("cannot open the pairing stream: {e}")))?;
        send.write_all(&secret)
            .await
            .map_err(|e| Error::Network(format!("cannot write the pairing secret: {e}")))?;
        send.finish()
            .map_err(|e| Error::Network(format!("cannot end the pairing stream: {e}")))?;
        let answer = recv
            .read_to_end(1)
            .await
            .map_err(|e| Error::Network(format!("cannot read the pairing answer: {e}")))?;
        Ok::<_, Error>(answer)
    }
    .await;
    conn.close(0u32.into(), b"redeemed");
    let _ = receiver.shutdown().await;
    match answer?.as_slice() {
        [0x00] => Ok(PairOutcome::Accepted(grant)),
        [0x02] => Ok(PairOutcome::Unreadable),
        // Any other byte is reserved, and SPEC.md section 11.2 says a client treats it as 0x01.
        _ => Ok(PairOutcome::Refused),
    }
}

/// Hands a delivery request for `text` to the daemon `to`, from `endpoint` (which is the send's own
/// endpoint and so the identity the daemon granted), with an optional top-level `name`, and answers
/// the one status byte. The ticket is secret and travels only on the stream.
pub async fn deliver_to(
    endpoint: &Endpoint,
    to: &PairedDaemon,
    text: &str,
    name: Option<&str>,
) -> Result<u8, Error> {
    let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
    let conn = super::super::receiver::dial(
        endpoint,
        to.endpoint_id,
        &to.addrs,
        to.relay.as_deref(),
        ALPN_DELIVER,
        events,
    )
    .await?;
    let answer = async {
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| Error::Network(format!("cannot open the delivery stream: {e}")))?;
        let mut request = Vec::new();
        write_varint(&mut request, text.len() as u64);
        request.extend_from_slice(text.as_bytes());
        match name {
            None => request.push(0),
            Some(name) => {
                request.push(1);
                write_varint(&mut request, name.len() as u64);
                request.extend_from_slice(name.as_bytes());
            }
        }
        send.write_all(&request)
            .await
            .map_err(|e| Error::Network(format!("cannot write the delivery: {e}")))?;
        send.finish()
            .map_err(|e| Error::Network(format!("cannot end the delivery stream: {e}")))?;
        let answer = recv
            .read_to_end(1)
            .await
            .map_err(|e| Error::Network(format!("cannot read the delivery answer: {e}")))?;
        Ok::<_, Error>(answer)
    }
    .await;
    conn.close(0u32.into(), b"delivered");
    match answer?.as_slice() {
        [status] => Ok(*status),
        // No status byte is a refusal: the daemon defines 0x00, 0x01 and 0x02 only.
        _ => Ok(0x01),
    }
}

/// The shortest LEB128 spelling of `value`, as `SPEC.md` section 2 requires.
fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let low = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(low);
            return;
        }
        out.push(low | 0x80);
    }
}
