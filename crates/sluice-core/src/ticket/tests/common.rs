use std::net::SocketAddr;

use crate::Error;

use super::super::{Body, Format, Ticket, base32};

pub(super) const ID: [u8; 32] = [0x11; 32];
pub(super) const ROOT: [u8; 32] = [0x22; 32];

pub(super) fn static_body(size: u64) -> Body {
    Body::Static {
        root: ROOT,
        size,
        format: Format::Raw,
    }
}

pub(super) fn live_body() -> Body {
    Body::Live {
        stream_id: [0x33; 16],
        sender_key: [0x44; 32],
    }
}

pub(super) fn addr(text: &str) -> SocketAddr {
    text.parse().expect("an address")
}

pub(super) fn text(bytes: &[u8]) -> String {
    format!("sluice1{}", base32::encode(bytes))
}

pub(super) fn varint(mut n: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// The bytes of a Static ticket, each field given as the bytes to write for it, so a test can put
/// one wrong field in an otherwise valid ticket.
pub(super) struct Parts {
    pub(super) version: u8,
    pub(super) kind: u8,
    pub(super) addrs: Vec<u8>,
    pub(super) relay: Vec<u8>,
    pub(super) expiry: Vec<u8>,
    pub(super) body: Vec<u8>,
}

impl Parts {
    pub(super) fn valid() -> Self {
        let mut body = ROOT.to_vec();
        body.extend(varint(300));
        body.push(0);
        Self {
            version: 1,
            kind: 0,
            addrs: vec![0],
            relay: vec![0],
            expiry: vec![0],
            body,
        }
    }

    pub(super) fn bytes(&self) -> Vec<u8> {
        let mut out = vec![self.version, self.kind];
        out.extend(ID);
        for part in [&self.addrs, &self.relay, &self.expiry, &self.body] {
            out.extend(part);
        }
        out
    }
}

/// The policy message of a ticket text that must be refused. It also checks that the message
/// does not hold the text, or any run of 8 characters of it.
pub(super) fn refusal(ticket_text: &str) -> String {
    let err = Ticket::decode(ticket_text).expect_err("the ticket must be refused");
    let Error::Policy(message) = err else {
        panic!("not a policy error: {err:?}");
    };
    let body = ticket_text.get(7..).unwrap_or_default();
    assert!(
        body.len() < 8 || !message.contains(body.get(..8).unwrap_or_default()),
        "{message}"
    );
    assert!(
        !message.contains(ticket_text) || ticket_text.len() < 8,
        "{message}"
    );
    message
}

pub(super) fn refusal_of(bytes: &[u8]) -> String {
    refusal(&text(bytes))
}
