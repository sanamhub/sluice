//! What a handler refuses with, and the status a client sees for each kind.

use sluice_core::Error;

/// Why a call failed. The message is the daemon's own sentence: it never holds a ticket.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ApiError {
    /// The request is malformed: an empty list, a ticket that does not decode, a path that is not
    /// there to read.
    #[error("{0}")]
    Invalid(String),
    /// A rule refused it: a path outside every root, a receive with no root, a ticket that is
    /// not for this kind of call.
    #[error("{0}")]
    Refused(String),
    /// No job has that id.
    #[error("{0}")]
    NotFound(String),
    /// A limit is reached: too many jobs or watchers at once.
    #[error("{0}")]
    Busy(String),
    /// The transfer could not reach the peer.
    #[error("{0}")]
    Unavailable(String),
    /// The work started and failed.
    #[error("{0}")]
    Failed(String),
}

/// What a ticket is replaced with if a message holds one.
const REDACTED: &str = "<ticket>";

impl ApiError {
    /// The engine's error as a call's error. Every character that could change what a terminal
    /// shows is escaped, and a message that holds the ticket's text has it replaced.
    pub(crate) fn from_core(error: &Error, ticket: &str) -> Self {
        let text = message_of(error, ticket);
        match error {
            Error::Policy(_) => Self::Refused(text),
            Error::Network(_) => Self::Unavailable(text),
            Error::Verify(_) | Error::Storage(_) | Error::Cancelled | Error::Bug(_) => {
                Self::Failed(text)
            }
        }
    }
}

/// The sentence a failed job reports: escaped as a peer's text is, and free of the ticket.
pub(crate) fn message_of(error: &Error, ticket: &str) -> String {
    let text = sluice_core::manifest::path::shown(&error.to_string());
    if ticket.is_empty() {
        text
    } else {
        text.replace(ticket, REDACTED)
    }
}

impl From<ApiError> for tonic::Status {
    fn from(error: ApiError) -> Self {
        let text = error.to_string();
        match error {
            ApiError::Invalid(_) => Self::invalid_argument(text),
            ApiError::Refused(_) => Self::permission_denied(text),
            ApiError::NotFound(_) => Self::not_found(text),
            ApiError::Busy(_) => Self::resource_exhausted(text),
            ApiError::Unavailable(_) => Self::unavailable(text),
            ApiError::Failed(_) => Self::internal(text),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_never_carries_the_ticket() {
        let error = Error::Policy("cannot use sluice1abcdef here".to_string());
        let made = ApiError::from_core(&error, "sluice1abcdef");
        assert_eq!(
            made,
            ApiError::Refused("policy denied: cannot use <ticket> here".to_string())
        );
    }

    #[test]
    fn a_control_character_in_a_message_is_escaped() {
        let error = Error::Storage("bad \u{1b}[2J name".to_string());
        let ApiError::Failed(text) = ApiError::from_core(&error, "") else {
            panic!("a storage error is a failure");
        };
        assert!(!text.contains('\u{1b}'), "{text:?}");
    }
}
