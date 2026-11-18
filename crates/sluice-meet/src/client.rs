//! The two client halves, driven by server frames. A transport sends what [`ClientEvent::Send`]
//! carries and prints what the other events say; the state machine never sees a socket or a clock.
//!
//! The sender knows the ticket and seals it. The receiver knows only the code and the sealed bytes
//! until they open. Neither side retries a code: a confirmation or seal failure sends `Fail` and
//! ends in [`ClientEvent::Burned`], which a caller must treat as a final answer.

use std::fmt;

use zeroize::Zeroizing;

use crate::code::Code;
use crate::crypto::{CryptoError, Exchange, Keys};
use crate::frame::{ClientFrame, ErrorCode, FailReason, Role, ServerFrame};

/// What a client state machine asks its transport or its caller to do next.
pub enum ClientEvent {
    /// Send this frame to the meeting place.
    Send(ClientFrame),
    /// The meeting place claimed a nameplate. [`Sender::code`] is now the code to print, and its
    /// words are the ones the sender chose.
    Claimed,
    /// The receiver opened the ticket. It is secret material; the caller must not log it.
    Ticket(Zeroizing<String>),
    /// The ticket was delivered and accepted.
    Done,
    /// The meeting is over because a guess failed or the peer left. The code must not be reused.
    Burned,
    /// The sender closed before delivering.
    PeerClosed,
    /// The meeting place refused the claim or join.
    Refused(ErrorCode),
}

impl fmt::Debug for ClientEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Send(frame) => f.debug_tuple("Send").field(frame).finish(),
            Self::Claimed => f.write_str("Claimed"),
            Self::Ticket(_) => f.write_str("Ticket(<redacted>)"),
            Self::Done => f.write_str("Done"),
            Self::Burned => f.write_str("Burned"),
            Self::PeerClosed => f.write_str("PeerClosed"),
            Self::Refused(code) => f.debug_tuple("Refused").field(code).finish(),
        }
    }
}

/// Why a client cannot continue. No variant holds a code, a word, a ticket, a key or a frame's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// The frame does not belong in the state the client is in.
    #[error("the meeting place sent a frame that does not belong here")]
    Unexpected,
    /// The plate the meeting place claimed is not the one the code names.
    #[error("the meeting place claimed another nameplate")]
    WrongNameplate,
    /// The meeting place gave this side the other role.
    #[error("the meeting place gave this side the wrong role")]
    WrongRole,
    /// SPAKE2 did not produce a key, or the peer's message was not one.
    #[error("the exchange did not produce a key")]
    Spake,
    /// The sealed ticket did not open.
    #[error("the sealed ticket did not open")]
    Seal,
}

impl From<CryptoError> for ClientError {
    fn from(error: CryptoError) -> Self {
        match error {
            CryptoError::Spake => Self::Spake,
            CryptoError::Aead | CryptoError::SealedLength | CryptoError::TicketNotUtf8 => {
                Self::Seal
            }
            _ => Self::Seal,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SenderState {
    AwaitingClaimed,
    AwaitingJoined,
    AwaitingSpake,
    AwaitingConfirm,
    AwaitingDone,
    Finished,
}

/// The sender: claims, runs side A, seals the ticket, and waits for the receiver's `Done`.
pub struct Sender {
    words: [u16; 2],
    code: Option<Code>,
    ticket: Zeroizing<String>,
    state: SenderState,
    exchange: Option<Exchange>,
    keys: Option<Keys>,
}

impl fmt::Debug for Sender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("code", &self.code)
            .field("state", &self.state)
            .field("ticket", &format_args!("<redacted>"))
            .finish()
    }
}

impl Sender {
    /// Starts a sender on two word indices and returns the `Claim` frame it must send first. The
    /// nameplate is not chosen here: the meeting place hands it out in `Claimed`, and the code is
    /// complete only then.
    ///
    /// # Errors
    ///
    /// [`crate::CodeError::Word`] when an index is past the BIP-0039 list.
    pub fn new(words: [u16; 2], ticket: &str) -> Result<(Self, ClientFrame), crate::CodeError> {
        // Validate the indices without choosing a nameplate; `new` only checks the word rule when
        // the placeholder is in range.
        crate::Code::new(1, words)?;
        let sender = Self {
            words,
            code: None,
            ticket: Zeroizing::new(ticket.to_string()),
            state: SenderState::AwaitingClaimed,
            exchange: None,
            keys: None,
        };
        Ok((sender, ClientFrame::Claim))
    }

    /// The code once the meeting place has claimed its nameplate.
    pub fn code(&self) -> Option<&Code> {
        self.code.as_ref()
    }

    /// Handles one frame from the meeting place.
    pub fn on_frame(&mut self, frame: ServerFrame) -> Result<Vec<ClientEvent>, ClientError> {
        match frame {
            ServerFrame::Error(code) => {
                self.state = SenderState::Finished;
                Ok(vec![ClientEvent::Refused(code)])
            }
            ServerFrame::Claimed(nameplate) => {
                if self.state != SenderState::AwaitingClaimed {
                    return Err(ClientError::Unexpected);
                }
                let code =
                    Code::new(nameplate, self.words).map_err(|_| ClientError::WrongNameplate)?;
                self.code = Some(code);
                self.state = SenderState::AwaitingJoined;
                Ok(vec![ClientEvent::Claimed])
            }
            ServerFrame::Joined(role) => {
                if role != Role::Sender {
                    return Err(ClientError::WrongRole);
                }
                if self.state != SenderState::AwaitingJoined {
                    return Err(ClientError::Unexpected);
                }
                let code = self.code.as_ref().ok_or(ClientError::WrongNameplate)?;
                let exchange = Exchange::new(Role::Sender, code);
                let outbound = exchange.outbound().to_vec();
                self.exchange = Some(exchange);
                self.state = SenderState::AwaitingSpake;
                Ok(vec![ClientEvent::Send(ClientFrame::Spake(outbound))])
            }
            ServerFrame::Spake(message) => {
                if self.state != SenderState::AwaitingSpake {
                    return Err(ClientError::Unexpected);
                }
                let exchange = self.exchange.take().ok_or(ClientError::Spake)?;
                let keys = exchange.finish(&message).map_err(ClientError::from)?;
                let tag = keys.confirmation(Role::Sender);
                self.keys = Some(keys);
                self.state = SenderState::AwaitingConfirm;
                Ok(vec![ClientEvent::Send(ClientFrame::Confirm(tag))])
            }
            ServerFrame::Confirm(tag) => {
                if self.state != SenderState::AwaitingConfirm {
                    return Err(ClientError::Unexpected);
                }
                let keys = self.keys.as_ref().ok_or(ClientError::Seal)?;
                if keys.verify(Role::Receiver, &tag).is_err() {
                    self.state = SenderState::Finished;
                    return Ok(vec![
                        ClientEvent::Send(ClientFrame::Fail(FailReason::ConfirmMismatch)),
                        ClientEvent::Burned,
                    ]);
                }
                let sealed = keys.seal(self.ticket.as_str()).map_err(ClientError::from)?;
                self.state = SenderState::AwaitingDone;
                Ok(vec![ClientEvent::Send(ClientFrame::Sealed(sealed))])
            }
            ServerFrame::Done => {
                if self.state != SenderState::AwaitingDone {
                    return Err(ClientError::Unexpected);
                }
                self.state = SenderState::Finished;
                Ok(vec![ClientEvent::Done])
            }
            ServerFrame::Burned => {
                self.state = SenderState::Finished;
                Ok(vec![ClientEvent::Burned])
            }
            ServerFrame::PeerClosed | ServerFrame::Sealed(_) => {
                self.state = SenderState::Finished;
                Ok(vec![ClientEvent::Burned])
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReceiverState {
    AwaitingJoined,
    AwaitingSpake,
    AwaitingConfirm,
    AwaitingSealed,
    Finished,
}

/// The receiver: joins, runs side B, checks the sender's tag, and opens the ticket.
#[derive(Debug)]
pub struct Receiver {
    code: Code,
    state: ReceiverState,
    exchange: Option<Exchange>,
    keys: Option<Keys>,
}

impl Receiver {
    /// Starts a receiver and returns the `Join` frame it must send first.
    pub fn new(code: Code) -> (Self, ClientFrame) {
        let receiver = Self {
            code,
            state: ReceiverState::AwaitingJoined,
            exchange: None,
            keys: None,
        };
        (receiver, ClientFrame::Join(code.nameplate()))
    }

    /// Handles one frame from the meeting place.
    pub fn on_frame(&mut self, frame: ServerFrame) -> Result<Vec<ClientEvent>, ClientError> {
        match frame {
            ServerFrame::Error(code) => {
                self.state = ReceiverState::Finished;
                Ok(vec![ClientEvent::Refused(code)])
            }
            ServerFrame::Joined(role) => {
                if role != Role::Receiver {
                    return Err(ClientError::WrongRole);
                }
                if self.state != ReceiverState::AwaitingJoined {
                    return Err(ClientError::Unexpected);
                }
                let exchange = Exchange::new(Role::Receiver, &self.code);
                let outbound = exchange.outbound().to_vec();
                self.exchange = Some(exchange);
                self.state = ReceiverState::AwaitingSpake;
                Ok(vec![ClientEvent::Send(ClientFrame::Spake(outbound))])
            }
            ServerFrame::Spake(message) => {
                if self.state != ReceiverState::AwaitingSpake {
                    return Err(ClientError::Unexpected);
                }
                let exchange = self.exchange.take().ok_or(ClientError::Spake)?;
                let keys = exchange.finish(&message).map_err(ClientError::from)?;
                let tag = keys.confirmation(Role::Receiver);
                self.keys = Some(keys);
                self.state = ReceiverState::AwaitingConfirm;
                Ok(vec![ClientEvent::Send(ClientFrame::Confirm(tag))])
            }
            ServerFrame::Confirm(tag) => {
                if self.state != ReceiverState::AwaitingConfirm {
                    return Err(ClientError::Unexpected);
                }
                let keys = self.keys.as_ref().ok_or(ClientError::Seal)?;
                if keys.verify(Role::Sender, &tag).is_err() {
                    self.state = ReceiverState::Finished;
                    return Ok(vec![
                        ClientEvent::Send(ClientFrame::Fail(FailReason::ConfirmMismatch)),
                        ClientEvent::Burned,
                    ]);
                }
                self.state = ReceiverState::AwaitingSealed;
                Ok(Vec::new())
            }
            ServerFrame::Sealed(sealed) => {
                if self.state != ReceiverState::AwaitingSealed {
                    return Err(ClientError::Unexpected);
                }
                let keys = self.keys.as_ref().ok_or(ClientError::Seal)?;
                let ticket = match keys.open(&sealed) {
                    Ok(ticket) => ticket,
                    Err(_) => {
                        self.state = ReceiverState::Finished;
                        return Ok(vec![
                            ClientEvent::Send(ClientFrame::Fail(FailReason::SealFailed)),
                            ClientEvent::Burned,
                        ]);
                    }
                };
                self.state = ReceiverState::Finished;
                Ok(vec![
                    ClientEvent::Send(ClientFrame::Done),
                    ClientEvent::Ticket(ticket),
                    ClientEvent::Done,
                ])
            }
            ServerFrame::Burned => {
                self.state = ReceiverState::Finished;
                Ok(vec![ClientEvent::Burned])
            }
            ServerFrame::PeerClosed => {
                self.state = ReceiverState::Finished;
                Ok(vec![ClientEvent::PeerClosed])
            }
            ServerFrame::Claimed(_) | ServerFrame::Done => Err(ClientError::Unexpected),
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sender => "sender",
            Self::Receiver => "receiver",
        })
    }
}

#[cfg(test)]
mod tests;
