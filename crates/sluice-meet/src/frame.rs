//! The frames of the meeting place. One WebSocket binary message is one frame; a text message is
//! refused.
//!
//! The bytes are `SPEC.md` section 4.1.2. Every decoder checks the tag, the exact length of the
//! body, the 4608-byte cap and the sealed value's own bounds, so a truncated, oversized, unknown or
//! ill-typed frame is a refusal and never a partial parse.

mod error;

use error::unknown_or_short;

/// The largest frame, in bytes.
pub const MAX_FRAME: usize = 4608;

/// The largest sealed value: a 4096-byte ticket, a 12-byte nonce and a 16-byte tag.
pub const MAX_SEALED: usize = 4124;

/// The largest ticket text.
pub const MAX_TICKET: usize = 4096;

/// The length of one SPAKE2 message.
pub const SPAKE_LEN: usize = 33;

/// Which side of a meeting a connection is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The side that claimed the nameplate and serves the content, SPAKE2 side A.
    Sender,
    /// The side that joined, SPAKE2 side B.
    Receiver,
}

impl Role {
    /// The byte the `Joined` frame carries.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Sender => 0,
            Self::Receiver => 1,
        }
    }

    /// The role a `Joined` byte names, or `None` for any other value.
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Sender),
            1 => Some(Self::Receiver),
            _ => None,
        }
    }
}

/// Why a client sent `Fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    /// The peer's confirmation tag did not match.
    ConfirmMismatch,
    /// The sealed ticket did not open.
    SealFailed,
}

impl FailReason {
    fn as_u8(self) -> u8 {
        match self {
            Self::ConfirmMismatch => 1,
            Self::SealFailed => 2,
        }
    }

    fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::ConfirmMismatch),
            2 => Some(Self::SealFailed),
            _ => None,
        }
    }
}

/// The refusal code in an `Error` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// No plate is held under that nameplate.
    NoCode,
    /// A receiver already joined the plate.
    Busy,
    /// The meeting place holds its maximum number of plates.
    Full,
    /// The plate outlived its TTL.
    Expired,
    /// The frame was larger than [`MAX_FRAME`].
    TooLarge,
    /// The frame was text, truncated, unknown or otherwise not a frame.
    BadFrame,
    /// The per-IP rate for claims and joins was spent.
    RateLimited,
}

impl ErrorCode {
    /// The byte the `Error` frame carries.
    pub fn as_u8(self) -> u8 {
        match self {
            Self::NoCode => 1,
            Self::Busy => 2,
            Self::Full => 3,
            Self::Expired => 4,
            Self::TooLarge => 5,
            Self::BadFrame => 6,
            Self::RateLimited => 7,
        }
    }

    /// The code a byte names, or `None` for any other value.
    pub fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::NoCode),
            2 => Some(Self::Busy),
            3 => Some(Self::Full),
            4 => Some(Self::Expired),
            5 => Some(Self::TooLarge),
            6 => Some(Self::BadFrame),
            7 => Some(Self::RateLimited),
            _ => None,
        }
    }
}

/// Every frame a client sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientFrame {
    /// Claim a nameplate.
    Claim,
    /// Join the named nameplate.
    Join(u32),
    /// One SPAKE2 message.
    Spake(Vec<u8>),
    /// The sender's or receiver's 32-byte confirmation tag.
    Confirm([u8; 32]),
    /// A nonce and a sealed ticket; only a sender sends it.
    Sealed(Vec<u8>),
    /// End the meeting because a tag or the seal failed.
    Fail(FailReason),
    /// The receiver accepted the ticket.
    Done,
}

/// Every frame the meeting place sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerFrame {
    /// The plate the claimant got.
    Claimed(u32),
    /// The role a peer plays in the meeting.
    Joined(Role),
    /// One SPAKE2 message from the peer.
    Spake(Vec<u8>),
    /// The peer's confirmation tag.
    Confirm([u8; 32]),
    /// The sealed ticket, to the receiver.
    Sealed(Vec<u8>),
    /// The meeting is over and the plate is gone.
    Burned,
    /// The sender closed before delivering.
    PeerClosed,
    /// The receiver accepted the ticket.
    Done,
    /// A refusal by code.
    Error(ErrorCode),
}

/// Why bytes are not a frame. The message names the rule and never quotes the bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// A WebSocket text message; only binary messages are frames.
    #[error("a text message is not a frame")]
    Text,
    /// Over [`MAX_FRAME`], or a sealed value over [`MAX_SEALED`].
    #[error("the frame is larger than the meeting place allows")]
    TooLarge,
    /// A known tag with too few bytes behind it.
    #[error("the frame is truncated")]
    Truncated,
    /// A tag this protocol does not define.
    #[error("the frame has an unknown tag")]
    Unknown,
    /// A known tag followed by more bytes than its body has.
    #[error("the frame has bytes after its body")]
    Trailing,
    /// A `Joined` role other than 0 or 1.
    #[error("the frame names a role that is not sender or receiver")]
    BadRole,
    /// A `Fail` reason other than 1 or 2.
    #[error("the frame names a failure reason that is not confirm or seal")]
    BadReason,
    /// An `Error` code other than 1 through 7.
    #[error("the frame names an error code that is not defined")]
    BadCode,
    /// A sealed value shorter than a nonce and a tag.
    #[error("the sealed value is shorter than a nonce and a tag")]
    BadSealed,
}

impl ClientFrame {
    /// Encodes the frame, refusing a sealed value or SPAKE2 message outside its bounds.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut out = Vec::new();
        match self {
            Self::Claim => out.push(0x01),
            Self::Join(nameplate) => {
                out.push(0x02);
                out.extend_from_slice(&nameplate.to_be_bytes());
            }
            Self::Spake(message) => {
                exactly(message.len(), SPAKE_LEN)?;
                out.push(0x03);
                out.extend_from_slice(message);
            }
            Self::Confirm(tag) => {
                out.push(0x04);
                out.extend_from_slice(tag);
            }
            Self::Sealed(sealed) => {
                sealed_length(sealed.len())?;
                out.push(0x05);
                out.extend_from_slice(sealed);
            }
            Self::Fail(reason) => {
                out.push(0x06);
                out.push(reason.as_u8());
            }
            Self::Done => out.push(0x07),
        }
        check_frame(out)
    }

    /// Decodes one binary message.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        Self::decode_message(false, bytes)
    }

    /// Decodes one message, refusing a text one before looking at the bytes.
    pub fn decode_message(text: bool, bytes: &[u8]) -> Result<Self, FrameError> {
        if text {
            return Err(FrameError::Text);
        }
        if bytes.len() > MAX_FRAME {
            return Err(FrameError::TooLarge);
        }
        match bytes.first().copied().ok_or(FrameError::Truncated)? {
            0x01 => {
                exactly(bytes.len(), 1)?;
                Ok(Self::Claim)
            }
            0x02 => Ok(Self::Join(u32::from_be_bytes(
                fixed(bytes, 4)?
                    .try_into()
                    .map_err(|_| FrameError::Truncated)?,
            ))),
            0x03 => Ok(Self::Spake(fixed(bytes, SPAKE_LEN)?.to_vec())),
            0x04 => Ok(Self::Confirm(
                fixed(bytes, 32)?
                    .try_into()
                    .map_err(|_| FrameError::Truncated)?,
            )),
            0x05 => {
                sealed_length(bytes.len().saturating_sub(1))?;
                Ok(Self::Sealed(bytes[1..].to_vec()))
            }
            0x06 => Ok(Self::Fail(
                FailReason::from_u8(fixed(bytes, 1)?[0]).ok_or(FrameError::BadReason)?,
            )),
            0x07 => {
                exactly(bytes.len(), 1)?;
                Ok(Self::Done)
            }
            tag => Err(unknown_or_short(tag)),
        }
    }
}

impl ServerFrame {
    /// Encodes the frame, refusing a sealed value or SPAKE2 message outside its bounds.
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        let mut out = Vec::new();
        match self {
            Self::Claimed(nameplate) => {
                out.push(0x81);
                out.extend_from_slice(&nameplate.to_be_bytes());
            }
            Self::Joined(role) => {
                out.push(0x82);
                out.push(role.as_u8());
            }
            Self::Spake(message) => {
                exactly(message.len(), SPAKE_LEN)?;
                out.push(0x83);
                out.extend_from_slice(message);
            }
            Self::Confirm(tag) => {
                out.push(0x84);
                out.extend_from_slice(tag);
            }
            Self::Sealed(sealed) => {
                sealed_length(sealed.len())?;
                out.push(0x85);
                out.extend_from_slice(sealed);
            }
            Self::Burned => out.push(0x86),
            Self::PeerClosed => out.push(0x87),
            Self::Done => out.push(0x88),
            Self::Error(code) => {
                out.push(0x8f);
                out.push(code.as_u8());
            }
        }
        check_frame(out)
    }

    /// Decodes one binary message.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        Self::decode_message(false, bytes)
    }

    /// Decodes one message, refusing a text one before looking at the bytes.
    pub fn decode_message(text: bool, bytes: &[u8]) -> Result<Self, FrameError> {
        if text {
            return Err(FrameError::Text);
        }
        if bytes.len() > MAX_FRAME {
            return Err(FrameError::TooLarge);
        }
        match bytes.first().copied().ok_or(FrameError::Truncated)? {
            0x81 => Ok(Self::Claimed(u32::from_be_bytes(
                fixed(bytes, 4)?
                    .try_into()
                    .map_err(|_| FrameError::Truncated)?,
            ))),
            0x82 => Ok(Self::Joined(
                Role::from_u8(fixed(bytes, 1)?[0]).ok_or(FrameError::BadRole)?,
            )),
            0x83 => Ok(Self::Spake(fixed(bytes, SPAKE_LEN)?.to_vec())),
            0x84 => Ok(Self::Confirm(
                fixed(bytes, 32)?
                    .try_into()
                    .map_err(|_| FrameError::Truncated)?,
            )),
            0x85 => {
                sealed_length(bytes.len().saturating_sub(1))?;
                Ok(Self::Sealed(bytes[1..].to_vec()))
            }
            0x86 => {
                exactly(bytes.len(), 1)?;
                Ok(Self::Burned)
            }
            0x87 => {
                exactly(bytes.len(), 1)?;
                Ok(Self::PeerClosed)
            }
            0x88 => {
                exactly(bytes.len(), 1)?;
                Ok(Self::Done)
            }
            0x8f => Ok(Self::Error(
                ErrorCode::from_u8(fixed(bytes, 1)?[0]).ok_or(FrameError::BadCode)?,
            )),
            tag => Err(unknown_or_short(tag)),
        }
    }
}

fn check_frame(bytes: Vec<u8>) -> Result<Vec<u8>, FrameError> {
    if bytes.len() > MAX_FRAME {
        return Err(FrameError::TooLarge);
    }
    Ok(bytes)
}

/// The body of `bytes` after its one-byte tag, which must be exactly `want` bytes long.
fn fixed(bytes: &[u8], want: usize) -> Result<&[u8], FrameError> {
    exactly(bytes.len(), 1 + want)?;
    Ok(&bytes[1..])
}

fn exactly(got: usize, want: usize) -> Result<(), FrameError> {
    match got.cmp(&want) {
        std::cmp::Ordering::Less => Err(FrameError::Truncated),
        std::cmp::Ordering::Greater => Err(FrameError::Trailing),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn sealed_length(len: usize) -> Result<(), FrameError> {
    if len < 28 {
        Err(FrameError::BadSealed)
    } else if len > MAX_SEALED {
        Err(FrameError::TooLarge)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
