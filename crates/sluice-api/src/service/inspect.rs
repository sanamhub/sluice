//! Reading what a ticket offers.

use sluice_core::engine::{Offering, inspect};

use super::{INSPECT_TIMEOUT, Service, decode_refusal, ticket_text};
use crate::error::ApiError;
use crate::proto;

/// How many names one answer lists. The rest are counted in `more_names`.
pub const MAX_NAMES: usize = 1000;

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
        let read = tokio::time::timeout(INSPECT_TIMEOUT, inspect(&text, Self::limits(), None));
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
    let names: Vec<proto::PeerName> = offered
        .entries
        .iter()
        .take(MAX_NAMES)
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
