//! The display text of a refusal code and the tag check both decoders share.

use std::fmt;

use super::{ErrorCode, FrameError};

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoCode => "no such code",
            Self::Busy => "someone is already meeting this code",
            Self::Full => "the meeting place is full",
            Self::Expired => "the code expired",
            Self::TooLarge => "the frame is too large",
            Self::BadFrame => "the frame is not a meeting-place frame",
            Self::RateLimited => "the meeting place is rate limited",
        })
    }
}

/// A tag this protocol defines but with too few bytes is truncated; any other tag is unknown.
pub(super) fn unknown_or_short(tag: u8) -> FrameError {
    if matches!(tag, 0x01..=0x07 | 0x81..=0x88 | 0x8f) {
        FrameError::Truncated
    } else {
        FrameError::Unknown
    }
}
