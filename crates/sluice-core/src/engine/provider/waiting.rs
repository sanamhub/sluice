//! What a one-download wait knows: the roots still owed, every root of the transfer (a completion
//! code is checked against all of them, not only the outstanding ones), the grace that starts when
//! the last root goes out, and which connections delivered or closed.

use std::collections::BTreeSet;

use iroh_blobs::Hash;

use super::{GRACE, policy};

pub(super) struct Waiting {
    remaining: BTreeSet<Hash>,
    pub(super) wanted: BTreeSet<Hash>,
    pub(super) grace: Option<std::pin::Pin<Box<tokio::time::Sleep>>>,
    closed: BTreeSet<u64>,
    connections: BTreeSet<u64>,
}

impl Waiting {
    pub(super) fn new(roots: Vec<Hash>) -> Result<Self, crate::Error> {
        let remaining: BTreeSet<Hash> = roots.into_iter().collect();
        if remaining.is_empty() {
            return Err(crate::Error::Bug(
                "a one-download send has no content to serve".to_string(),
            ));
        }
        Ok(Self {
            wanted: remaining.clone(),
            remaining,
            grace: None,
            closed: BTreeSet::new(),
            connections: BTreeSet::new(),
        })
    }

    /// One policy event: a delivered root is no longer owed, and a closed connection is recorded so
    /// the wait can end once every connection that delivered a root has gone. The grace is started
    /// when the last root goes out and never restarted, so a client that keeps asking for something
    /// else cannot hold the sender open for ever.
    pub(super) fn event(&mut self, event: policy::ServeEvent) {
        match event {
            policy::ServeEvent::Downloaded { hash, connection } => {
                if self.remaining.remove(&hash) {
                    self.connections.insert(connection);
                }
                if self.remaining.is_empty() && self.grace.is_none() {
                    self.grace = Some(Box::pin(tokio::time::sleep(GRACE)));
                }
            }
            policy::ServeEvent::Closed { connection } => {
                self.closed.insert(connection);
            }
        }
    }

    /// Whether every root is delivered and every connection that took one has closed: nothing is
    /// left to ask for, and nobody is left who could.
    pub(super) fn delivered(&self) -> bool {
        self.remaining.is_empty() && self.connections.is_subset(&self.closed)
    }
}
