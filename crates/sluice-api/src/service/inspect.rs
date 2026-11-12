//! Reading what a ticket offers.

use sluice_core::engine::{Offering, RelayModeChoice, inspect_with};

use super::{INSPECT_TIMEOUT, Service, decode_refusal, ticket_text};
use crate::error::ApiError;
use crate::proto;

/// How many names one answer lists. The rest are counted in `more_names`.
pub const MAX_NAMES: usize = 1000;

/// How many bytes of names one answer holds, which keeps the answer well under the 1 MiB a message
/// may be even when every name is as long as a path may be.
pub const MAX_NAME_BYTES: usize = 512 * 1024;

impl Service {
    /// Reads a ticket's manifest and says what it offers, without fetching content. A name in the
    /// answer is the peer's, in a message of its own type.
    pub async fn inspect_ticket(
        &self,
        request: proto::InspectTicketRequest,
    ) -> Result<proto::InspectTicketResponse, ApiError> {
        let text = ticket_text(request.ticket)?;
        sluice_core::ticket::Ticket::decode(&text).map_err(|e| decode_refusal(&e, &text))?;
        let _place = self.inner.inspects.try_acquire().map_err(|_| {
            ApiError::Busy("this daemon inspects a few tickets at once".to_string())
        })?;
        let read = tokio::time::timeout(
            INSPECT_TIMEOUT,
            inspect_with(
                &text,
                Self::limits(),
                None,
                matches!(self.inner.relay, RelayModeChoice::Loopback),
            ),
        );
        let offering = match read.await {
            Ok(Ok(offering)) => offering,
            Ok(Err(error)) => return Err(ApiError::from_core(&error, &text)),
            Err(_) => {
                return Err(ApiError::Unavailable(
                    "the sender did not answer in time".to_string(),
                ));
            }
        };
        Ok(answer(offering))
    }
}

fn answer(offering: Offering) -> proto::InspectTicketResponse {
    let Offering::Static(offered) = offering else {
        return proto::InspectTicketResponse {
            live: true,
            ..Default::default()
        };
    };
    let mut held = 0usize;
    let names: Vec<proto::PeerName> = offered
        .entries
        .iter()
        .take(MAX_NAMES)
        .take_while(|entry| {
            held += entry.path.len();
            held <= MAX_NAME_BYTES
        })
        .map(|entry| proto::PeerName {
            text: entry.path.clone(),
        })
        .collect();
    let more = offered.entries.len().saturating_sub(names.len());
    proto::InspectTicketResponse {
        files: offered.files,
        folders: offered.folders,
        links: offered.links,
        bytes: offered.bytes,
        names,
        more_names: more as u64,
        live: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sluice_core::engine::{Offered, OfferedEntry};
    use sluice_core::manifest::Kind;

    fn offering(names: usize, length: usize) -> Offering {
        let entries = (0..names)
            .map(|n| OfferedEntry {
                path: format!("{n:06}{}", "x".repeat(length - 6)),
                kind: Kind::File,
                size: 1,
            })
            .collect();
        Offering::Static(Offered {
            files: names as u64,
            folders: 0,
            links: 0,
            bytes: names as u64,
            all_files: names as u64,
            all_bytes: names as u64,
            filtered: false,
            entries,
        })
    }

    #[test]
    fn names_stop_at_the_byte_budget_and_the_rest_are_counted() {
        // 800 names of 4000 bytes would be 3.2 MB, over a message.
        let answered = answer(offering(800, 4000));
        let bytes: usize = answered.names.iter().map(|n| n.text.len()).sum();
        assert!(bytes <= MAX_NAME_BYTES, "{bytes} bytes of names");
        assert_eq!(answered.names.len(), MAX_NAME_BYTES / 4000);
        assert_eq!(answered.names.len() as u64 + answered.more_names, 800);
        let encoded = prost::Message::encoded_len(&answered);
        assert!(
            encoded < crate::MAX_MESSAGE_BYTES,
            "{encoded} bytes in the answer"
        );
    }

    #[test]
    fn a_short_list_is_whole_and_a_long_one_is_cut_at_the_name_limit() {
        let few = answer(offering(10, 20));
        assert_eq!((few.names.len(), few.more_names), (10, 0));
        let many = answer(offering(MAX_NAMES + 50, 20));
        assert_eq!((many.names.len(), many.more_names), (MAX_NAMES, 50));
    }
}
