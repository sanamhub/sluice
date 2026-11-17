//! The frames of the meeting place. One WebSocket binary message is one frame.

use std::fmt;

/// Larger than a 4096-byte ticket plus a 12-byte nonce and a 16-byte tag, with room for a frame.
pub const MAX_FRAME: usize = 4608;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Sender = 0,
    Receiver = 1,
}

impl Role {
    pub fn from_wire(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Sender),
            1 => Some(Self::Receiver),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailReason {
    ConfirmMismatch = 1,
    SealFailed = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    NoCode = 1,
    Busy = 2,
    Full = 3,
    Expired = 4,
    TooLarge = 5,
    BadFrame = 6,
    RateLimited = 7,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    Claim,
    Join(u32),
    Spake(Vec<u8>),
    Confirm([u8; 32]),
    Sealed(Vec<u8>),
    Fail(FailReason),
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerMsg {
    Claimed(u32),
    Joined(Role),
    Spake(Vec<u8>),
    Confirm([u8; 32]),
    Sealed(Vec<u8>),
    Burned,
    PeerClosed,
    Done,
    Error(ErrorCode),
}

impl ClientMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Claim => out.push(0x01),
            Self::Join(plate) => {
                out.push(0x02);
                out.extend_from_slice(&plate.to_be_bytes());
            }
            Self::Spake(msg) => {
                out.push(0x03);
                out.extend_from_slice(msg);
            }
            Self::Confirm(tag) => {
                out.push(0x04);
                out.extend_from_slice(tag);
            }
            Self::Sealed(sealed) => {
                out.push(0x05);
                out.extend_from_slice(sealed);
            }
            Self::Fail(reason) => {
                out.push(0x06);
                out.push(*reason as u8);
            }
            Self::Done => out.push(0x07),
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_FRAME {
            return None;
        }
        match *bytes.first()? {
            0x01 if bytes.len() == 1 => Some(Self::Claim),
            0x02 if bytes.len() == 5 => {
                Some(Self::Join(u32::from_be_bytes(bytes[1..5].try_into().ok()?)))
            }
            0x03 if bytes.len() > 1 => Some(Self::Spake(bytes[1..].to_vec())),
            0x04 if bytes.len() == 33 => {
                Some(Self::Confirm(bytes[1..].try_into().ok()?))
            }
            0x05 if bytes.len() > 13 => Some(Self::Sealed(bytes[1..].to_vec())),
            0x06 if bytes.len() == 2 => Some(Self::Fail(match bytes[1] {
                1 => FailReason::ConfirmMismatch,
                2 => FailReason::SealFailed,
                _ => return None,
            })),
            0x07 if bytes.len() == 1 => Some(Self::Done),
            _ => None,
        }
    }
}

impl ServerMsg {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Self::Claimed(plate) => {
                out.push(0x81);
                out.extend_from_slice(&plate.to_be_bytes());
            }
            Self::Joined(role) => {
                out.push(0x82);
                out.push(*role as u8);
            }
            Self::Spake(msg) => {
                out.push(0x83);
                out.extend_from_slice(msg);
            }
            Self::Confirm(tag) => {
                out.push(0x84);
                out.extend_from_slice(tag);
            }
            Self::Sealed(sealed) => {
                out.push(0x85);
                out.extend_from_slice(sealed);
            }
            Self::Burned => out.push(0x86),
            Self::PeerClosed => out.push(0x87),
            Self::Done => out.push(0x88),
            Self::Error(code) => {
                out.push(0x8f);
                out.push(*code as u8);
            }
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_FRAME {
            return None;
        }
        match *bytes.first()? {
            0x81 if bytes.len() == 5 => {
                Some(Self::Claimed(u32::from_be_bytes(bytes[1..5].try_into().ok()?)))
            }
            0x82 if bytes.len() == 2 => Some(Self::Joined(Role::from_wire(bytes[1])?)),
            0x83 if bytes.len() > 1 => Some(Self::Spake(bytes[1..].to_vec())),
            0x84 if bytes.len() == 33 => Some(Self::Confirm(bytes[1..].try_into().ok()?)),
            0x85 if bytes.len() > 13 => Some(Self::Sealed(bytes[1..].to_vec())),
            0x86 if bytes.len() == 1 => Some(Self::Burned),
            0x87 if bytes.len() == 1 => Some(Self::PeerClosed),
            0x88 if bytes.len() == 1 => Some(Self::Done),
            0x8f if bytes.len() == 2 => Some(Self::Error(match bytes[1] {
                1 => ErrorCode::NoCode,
                2 => ErrorCode::Busy,
                3 => ErrorCode::Full,
                4 => ErrorCode::Expired,
                5 => ErrorCode::TooLarge,
                6 => ErrorCode::BadFrame,
                7 => ErrorCode::RateLimited,
                _ => return None,
            })),
            _ => None,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::NoCode => "no such code",
            Self::Busy => "code already joined",
            Self::Full => "relay full",
            Self::Expired => "code expired",
            Self::TooLarge => "frame too large",
            Self::BadFrame => "bad frame",
            Self::RateLimited => "rate limited",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let cases = [
            ClientMsg::Claim,
            ClientMsg::Join(42),
            ClientMsg::Spake(vec![1, 2, 3]),
            ClientMsg::Confirm([7; 32]),
            ClientMsg::Sealed(vec![0; 40]),
            ClientMsg::Fail(FailReason::ConfirmMismatch),
            ClientMsg::Done,
        ];
        for msg in cases {
            assert_eq!(ClientMsg::decode(&msg.encode()), Some(msg));
        }
        let cases = [
            ServerMsg::Claimed(42),
            ServerMsg::Joined(Role::Sender),
            ServerMsg::Spake(vec![1, 2, 3]),
            ServerMsg::Confirm([7; 32]),
            ServerMsg::Sealed(vec![0; 40]),
            ServerMsg::Burned,
            ServerMsg::PeerClosed,
            ServerMsg::Done,
            ServerMsg::Error(ErrorCode::Busy),
        ];
        for msg in cases {
            assert_eq!(ServerMsg::decode(&msg.encode()), Some(msg));
        }
    }

    #[test]
    fn an_oversized_or_truncated_frame_is_refused() {
        assert_eq!(ServerMsg::decode(&[0x83; MAX_FRAME + 1]), None);
        assert_eq!(ClientMsg::decode(&[]), None);
        assert_eq!(ClientMsg::decode(&[0x04, 1, 2]), None);
        assert_eq!(ClientMsg::decode(&[0x99]), None);
    }
}
