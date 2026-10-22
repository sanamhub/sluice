//! Why a record was refused or a stream aborted, by the numbered rules of `SPEC.md` section 6.

/// A rule of `SPEC.md` section 6 that a record breaks.
///
/// The text names the rule and nothing of the record, so it can go to a log as it is.
///
/// # Examples
///
/// ```
/// use sluice_live::Fault;
///
/// assert_eq!(Fault::Fork.rule(), 7);
/// assert!(Fault::Fork.to_string().ends_with("(live rule 7)"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Fault {
    /// The record is not the length its table gives.
    #[error("the record is not the length its table gives (live rule 1)")]
    Length,
    /// The `LogOpen` names another `stream_id` or `sender_key` than the ticket.
    #[error("the opening record names another stream or sender than the ticket (live rule 2)")]
    Ticket,
    /// The signature does not verify under the ticket's key, strictly, with its label and stream,
    /// or that key is not in its one encoding or is of small order.
    #[error("the signature does not verify, or the ticket's key is not a valid one (live rule 3)")]
    Signature,
    /// `segment_bytes` is not a multiple of 16,384 from 1 MiB to 64 MiB.
    #[error("segment_bytes is outside its bounds (live rule 4)")]
    SegmentBytes,
    /// The frame's `seq` is past the next one.
    #[error("the frame is ahead of the next seq (live rule 5)")]
    Ahead,
    /// A second, validly signed entry for a `seq` that already has one.
    #[error("the sender signed two entries for one seq (live rule 7)")]
    Fork,
    /// The entry's `prev_entry_hash` is not the head of the chain.
    #[error("prev_entry_hash is not the head of the chain (live rule 8)")]
    PrevHash,
    /// The entry's `len` is 0 or more than `segment_bytes`.
    #[error("len is 0 or more than segment_bytes (live rule 9)")]
    Len,
    /// The entry comes after one with a `len` below `segment_bytes`, which was the last.
    #[error("an entry follows a short one (live rule 10)")]
    AfterShort,
    /// The entry comes after the `LogClose`.
    #[error("an entry follows the close (live rule 11)")]
    AfterClose,
    /// The `LogClose` counts entries the receiver does not have yet.
    #[error("the close counts entries that have not arrived (live rule 12)")]
    CloseAhead,
    /// The `LogClose`'s `total_len` is not the sum of the entries' `len`.
    #[error("total_len is not the sum of the entries (live rule 13)")]
    TotalLen,
    /// The `LogClose`'s `last_entry_hash` is not the head of the chain.
    #[error("last_entry_hash is not the head of the chain (live rule 14)")]
    LastEntry,
    /// A second, validly signed `LogClose` with other bytes.
    #[error("the sender signed two closes (live rule 15)")]
    SecondClose,
}

impl Fault {
    /// The number of the rule in `SPEC.md` section 6.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(sluice_live::Fault::Length.rule(), 1);
    /// ```
    pub fn rule(self) -> u8 {
        match self {
            Self::Length => 1,
            Self::Ticket => 2,
            Self::Signature => 3,
            Self::SegmentBytes => 4,
            Self::Ahead => 5,
            Self::Fork => 7,
            Self::PrevHash => 8,
            Self::Len => 9,
            Self::AfterShort => 10,
            Self::AfterClose => 11,
            Self::CloseAhead => 12,
            Self::TotalLen => 13,
            Self::LastEntry => 14,
            Self::SecondClose => 15,
        }
    }
}

/// The errors of this crate. Two of the six names in `AGENTS.md`, and nothing else.
///
/// # Examples
///
/// ```
/// use sluice_live::{Error, Fault, LogOpen};
///
/// assert_eq!(LogOpen::decode(&[0; 10]), Err(Error::Verify(Fault::Length)));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A record broke a rule, so it was refused.
    #[error("a live record was refused: {0}")]
    Verify(Fault),
    /// A call to the sender would have signed a record that breaks the rule.
    #[error("the sender will not sign that: {0}")]
    Policy(Fault),
}
