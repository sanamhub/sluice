//! The refusal of a connection that did not negotiate the post-quantum key exchange group
//! (ADR 0018): the check a dialling side makes on the connection it holds, and the hook a listening
//! endpoint runs on every connection it accepts.

use iroh::endpoint::{AfterHandshakeOutcome, Connection, EndpointHooks, Side, VarInt};
use tokio::sync::mpsc::UnboundedSender;

use super::Receiver;
use crate::Error;

/// The key exchange group every connection has to negotiate (ADR 0018).
const REQUIRED_GROUP: rustls::NamedGroup = rustls::NamedGroup::X25519MLKEM768;

/// The group the handshake settled on, when this build's TLS recorded one.
pub(in crate::engine) fn negotiated_group(conn: &Connection) -> Option<rustls::NamedGroup> {
    let data = conn.handshake_data()?;
    let data = data.downcast_ref::<noq::crypto::rustls::HandshakeData>()?;
    data.negotiated_key_exchange_group
}

/// What a peer is told, and what the listener prints, when its handshake is not post-quantum.
fn not_post_quantum(group: Option<rustls::NamedGroup>) -> String {
    match group {
        Some(group) => format!(
            "the connection negotiated {group:?}, and Sluice requires X25519MLKEM768, so it was \
             closed"
        ),
        None => "the connection reported no key exchange group, and Sluice requires \
                 X25519MLKEM768, so it was closed"
            .to_string(),
    }
}

/// Refuses a connection this process dialled whose handshake is not post-quantum. The dialling
/// side holds the connection, so it can name the group in the error its user reads and send the
/// peer the policy close.
pub(in crate::engine) fn require_post_quantum(conn: &Connection) -> Result<(), Error> {
    let group = negotiated_group(conn);
    if group == Some(REQUIRED_GROUP) {
        return Ok(());
    }
    let reason = not_post_quantum(group);
    conn.close(VarInt::from(crate::engine::POLICY_CLOSE), reason.as_bytes());
    Err(Error::Policy(reason))
}

/// Refuses a connection whose handshake is not post-quantum, on the side that accepts it.
///
/// A listener's protocol handlers are not ours to wrap: the blob protocol belongs to
/// `iroh-blobs`, and it answers a request as soon as it holds the connection. The endpoint's hook
/// is the one place every accepted connection passes before any protocol can answer, so the check
/// lives here, and no path through the router can miss it. A dialled connection is checked by
/// [`super::Receiver`], which holds the connection and can name the group in the error its user
/// reads.
#[derive(Debug)]
pub(super) struct PostQuantum {
    /// Where the serve loop of a provider collects the line about a refused peer, when this
    /// endpoint belongs to one.
    pub(super) report: Option<UnboundedSender<Receiver>>,
}

impl EndpointHooks for PostQuantum {
    async fn after_handshake<'a>(&'a self, conn: &'a Connection) -> AfterHandshakeOutcome {
        if conn.side() != Side::Server {
            return AfterHandshakeOutcome::accept();
        }
        let group = negotiated_group(conn);
        if group == Some(REQUIRED_GROUP) {
            return AfterHandshakeOutcome::accept();
        }
        if let Some(report) = &self.report {
            let _ = report.send(Receiver::RefusedGroup {
                endpoint: *conn.remote_id().as_bytes(),
                group: group.map(|group| format!("{group:?}")),
            });
        }
        AfterHandshakeOutcome::Reject {
            error_code: VarInt::from(crate::engine::POLICY_CLOSE),
            reason: not_post_quantum(group).into_bytes(),
        }
    }
}
