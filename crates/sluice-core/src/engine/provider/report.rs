//! The line a receiver's report is worth. A limited send names a receiver when it takes a place and
//! names a refused endpoint once; an unlimited send names every connection as it opens and closes.

use std::collections::BTreeSet;

use iroh_blobs::Hash;

use super::{Provider, lock_places};
use crate::engine::{Event, emit, endpoint};

impl Provider {
    /// Reports a receiver's report as its line. `limited` is whether the send offers places, which
    /// is what decides between a line per connection and a line per receiver.
    pub(super) fn report(
        &self,
        reported: endpoint::Receiver,
        limited: bool,
        roots: &BTreeSet<Hash>,
        events: &tokio::sync::mpsc::UnboundedSender<Event>,
    ) {
        let event = match reported {
            endpoint::Receiver::Opened { .. } if limited => return,
            endpoint::Receiver::Opened {
                endpoint, relayed, ..
            }
            | endpoint::Receiver::Placed { endpoint, relayed } => Event::ReceiverStarted {
                endpoint: Self::hex(&endpoint),
                relayed,
            },
            endpoint::Receiver::Refused {
                endpoint,
                relayed,
                places,
            } => Event::ReceiverRefused {
                endpoint: Self::hex(&endpoint),
                relayed,
                places,
            },
            endpoint::Receiver::Closed {
                connection,
                endpoint,
                relayed,
                claimed_whole,
            } => {
                if limited && !lock_places(&self.places).holds(&endpoint) {
                    return;
                }
                Event::ReceiverFinished {
                    endpoint: Self::hex(&endpoint),
                    relayed,
                    whole: claimed_whole && self.believed(connection, roots),
                }
            }
        };
        emit(events, event);
    }

    /// Whether a report says a connection opened or closed, for a set of the connections still open.
    pub(super) fn track(reported: &endpoint::Receiver, opened: &mut BTreeSet<u64>) {
        match reported {
            endpoint::Receiver::Opened { connection, .. } => {
                opened.insert(*connection);
            }
            endpoint::Receiver::Closed { connection, .. } => {
                opened.remove(connection);
            }
            _ => {}
        }
    }

    /// An endpoint id as the 64 lower-case hex characters a ticket and `sluice id` spell it with.
    fn hex(bytes: &[u8; 32]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
