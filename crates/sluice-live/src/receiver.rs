//! The receiver's side: it checks every record a provider gives against the numbered rules of
//! `SPEC.md` section 6, and says what to do with it.

use ed25519_dalek::VerifyingKey;

use crate::sig::{self, Label};
use crate::{Error, Fault, Frame, LogClose, LogEntry, LogOpen, segment_bytes_in_bounds};

/// What the receiver made of one record.
///
/// # Examples
///
/// ```
/// use sluice_live::{Fault, Outcome};
///
/// let refused = Outcome::Refused(Fault::Signature);
/// assert_ne!(refused, Outcome::Repeat { seq: 0 });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The entry is the next link of the chain. Its segment can be fetched and kept.
    Accepted {
        /// The entry's place in the chain.
        seq: u64,
        /// The root its segment is fetched and verified by.
        segment_root: [u8; 32],
        /// The length of the segment.
        len: u32,
    },
    /// The frame repeats an accepted entry and was dropped.
    Repeat {
        /// The entry it repeats.
        seq: u64,
    },
    /// The record was refused and changed nothing. Another provider can give the right one.
    Refused(Fault),
    /// The stream is closed. The caller hashes the file it assembled and compares the hash with
    /// `final_root`, which this crate has no bytes to do.
    Closed {
        /// The BLAKE3 root of the whole file.
        final_root: [u8; 32],
        /// The length of the whole file, which the entries add up to.
        total_len: u64,
    },
    /// The sender broke the chain. Only the segments of entries before `seq` are kept.
    Aborted {
        /// The first entry that is not kept.
        seq: u64,
        /// The rule the sender broke.
        reason: Fault,
    },
}

/// Where the stream stands.
///
/// # Examples
///
/// See [`Receiver`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Entries are still arriving.
    Open {
        /// The `seq` of the next entry, which is the number of entries accepted.
        next_seq: u64,
    },
    /// A `LogClose` was accepted.
    Closed {
        /// The BLAKE3 root of the whole file.
        final_root: [u8; 32],
        /// The length of the whole file.
        total_len: u64,
    },
    /// The stream was aborted.
    Aborted {
        /// The first entry that is not kept.
        seq: u64,
        /// The rule the sender broke.
        reason: Fault,
    },
}

/// Checks the records of one stream. A refused record leaves it as it was. Once aborted it
/// answers every call with the abort, and once closed it accepts nothing.
///
/// It keeps the 32-byte hash of every accepted entry, to tell a repeat from a fork.
///
/// # Examples
///
/// ```
/// use ed25519_dalek::SigningKey;
/// use sluice_live::{Fault, Outcome, Receiver, Sender, Status};
///
/// // The secret key of RFC 8032 section 7.1, TEST 1, which is published.
/// let key = SigningKey::from_bytes(&[
///     0x9d, 0x61, 0xb1, 0x9d, 0xef, 0xfd, 0x5a, 0x60, 0xba, 0x84, 0x4a, 0xf4, 0x92, 0xec, 0x2c,
///     0xc4, 0x44, 0x49, 0xc5, 0x69, 0x7b, 0x32, 0x69, 0x19, 0x70, 0x3b, 0xac, 0x03, 0x1c, 0xae,
///     0x7f, 0x60,
/// ]);
/// let sender_key = key.verifying_key().to_bytes();
/// let mut sender = Sender::new(key, [7; 16], 1 << 20, 0)?;
/// let open = sender.log_open().encode();
/// let frame = sender.seal([1; 32], 100)?.encode();
/// let close = sender.close([2; 32], 0).encode();
///
/// // The stream id and the key come from the ticket, the records from a provider.
/// let mut receiver = Receiver::new([7; 16], sender_key, &open)?;
/// assert_eq!(receiver.close(&close), Outcome::Refused(Fault::CloseAhead));
/// assert_eq!(
///     receiver.frame(&frame),
///     Outcome::Accepted { seq: 0, segment_root: [1; 32], len: 100 }
/// );
/// assert_eq!(receiver.frame(&frame), Outcome::Repeat { seq: 0 });
/// let mut damaged = frame;
/// damaged[8] ^= 1;
/// assert_eq!(receiver.frame(&damaged), Outcome::Refused(Fault::Signature));
/// assert_eq!(
///     receiver.close(&close),
///     Outcome::Closed { final_root: [2; 32], total_len: 100 }
/// );
/// assert_eq!(receiver.status(), Status::Closed { final_root: [2; 32], total_len: 100 });
/// # Ok::<(), sluice_live::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Receiver {
    stream_id: [u8; 16],
    key: VerifyingKey,
    segment_bytes: u32,
    open_hash: [u8; 32],
    hashes: Vec<[u8; 32]>,
    /// The sum of the accepted `len`. A `u128` cannot wrap: there are fewer than 2^64 entries
    /// of fewer than 2^32 bytes.
    total: u128,
    last_short: bool,
    state: State,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    Open,
    Closed(LogClose),
    Aborted { seq: u64, reason: Fault },
}

impl Receiver {
    /// A receiver for the stream that a ticket names by `stream_id` and `sender_key`, started
    /// from the `LogOpen` in `open`.
    ///
    /// # Errors
    ///
    /// [`Error::Verify`] when `open` breaks rule 1, 2, 3 or 4. Another provider can give the
    /// right one, unless the fault is that `sender_key` is not a valid key, which rule 3 also
    /// refuses.
    ///
    /// # Examples
    ///
    /// See [`Receiver`].
    pub fn new(stream_id: [u8; 16], sender_key: [u8; 32], open: &[u8]) -> Result<Self, Error> {
        let record = LogOpen::decode(open)?;
        if record.stream_id != stream_id || record.sender_key != sender_key {
            return Err(Error::Verify(Fault::Ticket));
        }
        // No signature verifies under a key that is no point, is not in its one encoding, or is of
        // small order, so such a key is refused the way a bad signature is, under rule 3.
        let key = sig::sender_key(&sender_key).ok_or(Error::Verify(Fault::Signature))?;
        if !sig::verifies(&key, Label::Open, &stream_id, &record.signed(), &record.sig) {
            return Err(Error::Verify(Fault::Signature));
        }
        if !segment_bytes_in_bounds(record.segment_bytes) {
            return Err(Error::Verify(Fault::SegmentBytes));
        }
        Ok(Self {
            stream_id,
            key,
            segment_bytes: record.segment_bytes,
            open_hash: record.hash(),
            hashes: Vec::new(),
            total: 0,
            last_short: false,
            state: State::Open,
        })
    }

    /// The size of a full segment, from the `LogOpen`.
    ///
    /// # Examples
    ///
    /// See [`Receiver`].
    pub fn segment_bytes(&self) -> u32 {
        self.segment_bytes
    }

    /// Where the stream stands.
    ///
    /// # Examples
    ///
    /// See [`Receiver`].
    pub fn status(&self) -> Status {
        match self.state {
            State::Open => Status::Open {
                next_seq: self.next_seq(),
            },
            State::Closed(close) => Status::Closed {
                final_root: close.final_root,
                total_len: close.total_len,
            },
            State::Aborted { seq, reason } => Status::Aborted { seq, reason },
        }
    }

    /// Checks the frame in `bytes`, by rules 1, 3, 5 to 11.
    ///
    /// # Examples
    ///
    /// See [`Receiver`].
    pub fn frame(&mut self, bytes: &[u8]) -> Outcome {
        if let State::Aborted { seq, reason } = self.state {
            return Outcome::Aborted { seq, reason };
        }
        let outcome = self.check_frame(bytes);
        self.settle(outcome)
    }

    /// Checks the `LogClose` in `bytes`, by rules 1, 3 and 12 to 15.
    ///
    /// # Examples
    ///
    /// See [`Receiver`].
    pub fn close(&mut self, bytes: &[u8]) -> Outcome {
        if let State::Aborted { seq, reason } = self.state {
            return Outcome::Aborted { seq, reason };
        }
        let outcome = self.check_close(bytes);
        self.settle(outcome)
    }

    fn next_seq(&self) -> u64 {
        self.hashes.len() as u64
    }

    fn head(&self) -> [u8; 32] {
        self.hashes.last().copied().unwrap_or(self.open_hash)
    }

    fn verifies(&self, label: Label, body: &[u8], signature: &[u8; 64]) -> bool {
        sig::verifies(&self.key, label, &self.stream_id, body, signature)
    }

    /// An abort sticks. After a close, anything that does not abort is answered with the close.
    fn settle(&mut self, outcome: Outcome) -> Outcome {
        match (&self.state, outcome) {
            (_, Outcome::Aborted { seq, reason }) => {
                self.state = State::Aborted { seq, reason };
                // Nothing at or after `seq` is kept, and nothing more is accepted.
                self.hashes = Vec::new();
                outcome
            }
            (State::Closed(close), _) => Outcome::Closed {
                final_root: close.final_root,
                total_len: close.total_len,
            },
            _ => outcome,
        }
    }

    fn check_frame(&mut self, bytes: &[u8]) -> Outcome {
        let Ok(frame) = Frame::decode(bytes) else {
            return Outcome::Refused(Fault::Length);
        };
        let seq = frame.entry.seq;
        let next = self.next_seq();
        if seq > next {
            return Outcome::Refused(Fault::Ahead);
        }
        let encoded = frame.entry.encode();
        if seq < next {
            // Below the length of the list, so it fits a usize.
            if self.hashes[seq as usize] == *blake3::hash(&encoded).as_bytes() {
                return Outcome::Repeat { seq };
            }
            if !self.verifies(Label::Entry, &encoded, &frame.sig) {
                return Outcome::Refused(Fault::Signature);
            }
            return Outcome::Aborted {
                seq,
                reason: Fault::Fork,
            };
        }
        if !self.verifies(Label::Entry, &encoded, &frame.sig) {
            return Outcome::Refused(Fault::Signature);
        }
        match self.chain_fault(&frame.entry) {
            Some(reason) => Outcome::Aborted { seq, reason },
            None => self.accept(&frame.entry),
        }
    }

    /// Rules 8 to 11, for a validly signed entry at the next `seq`.
    fn chain_fault(&self, entry: &LogEntry) -> Option<Fault> {
        if matches!(self.state, State::Closed(_)) {
            Some(Fault::AfterClose)
        } else if entry.prev_entry_hash != self.head() {
            Some(Fault::PrevHash)
        } else if entry.len == 0 || entry.len > self.segment_bytes {
            Some(Fault::Len)
        } else if self.last_short {
            Some(Fault::AfterShort)
        } else {
            None
        }
    }

    fn accept(&mut self, entry: &LogEntry) -> Outcome {
        self.hashes.push(entry.hash());
        self.total += u128::from(entry.len);
        self.last_short = entry.len < self.segment_bytes;
        Outcome::Accepted {
            seq: entry.seq,
            segment_root: entry.segment_root,
            len: entry.len,
        }
    }

    fn check_close(&mut self, bytes: &[u8]) -> Outcome {
        let Ok(close) = LogClose::decode(bytes) else {
            return Outcome::Refused(Fault::Length);
        };
        let kept = match self.state {
            State::Closed(kept) => Some(kept),
            _ => None,
        };
        if kept == Some(close) {
            return Outcome::Closed {
                final_root: close.final_root,
                total_len: close.total_len,
            };
        }
        if !self.verifies(Label::Close, &close.signed(), &close.sig) {
            return Outcome::Refused(Fault::Signature);
        }
        let seq = self.next_seq();
        let abort = |reason| Outcome::Aborted { seq, reason };
        if kept.is_some() {
            return abort(Fault::SecondClose);
        }
        let total = u128::from(close.total_len);
        if total > self.total && !self.last_short {
            return Outcome::Refused(Fault::CloseAhead);
        }
        if total != self.total {
            return abort(Fault::TotalLen);
        }
        if close.last_entry_hash != self.head() {
            return abort(Fault::LastEntry);
        }
        self.state = State::Closed(close);
        Outcome::Closed {
            final_root: close.final_root,
            total_len: close.total_len,
        }
    }
}
