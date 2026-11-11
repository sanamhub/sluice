//! Delta hints: the content-defined chunks of a file, the signatures of those chunks, and the
//! ranges of a target that a requester still has to fetch. `SPEC.md` section 7 states the shapes
//! and the limits this crate enforces.
//!
//! Bytes in, values out: no file, no socket, no async (ADR 0008). The engine chunks the content it
//! serves and the candidate a receiver already holds with the same parameters, matches strong
//! hashes, and fetches what is left. Nothing here decides that a result is right: the assembled
//! file is verified against its BLAKE3 root, so a wrong or hostile hint costs time and nothing
//! else.

mod chunker;
mod codec;
mod matcher;

pub use chunker::{Params, SMALLEST, gear_entry};
pub use codec::{Entry, MAX_ENTRIES, NeedRanges, Range, SigList};
pub use matcher::{Candidate, Plan, Span, plan};

/// The average chunk size this release writes when nothing else is asked for: 8 KiB. The smallest
/// file worth a delta is [`chunker::SMALLEST`]; below it a plain fetch costs less than the hint.
pub const AVG_BYTES: u32 = 8 * 1024;

/// What a decoder refuses, as a rule and never as bytes. A hint comes from a peer, so the message
/// names the rule and nothing of the message it refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// An average chunk size that is not a power of two within its bounds.
    Average,
    /// A length that a chunk of this algorithm cannot have.
    ChunkLength,
    /// A count that is more than a message may carry.
    TooManyEntries,
    /// A field the message does not have enough bytes for.
    Truncated,
    /// A varint that is not in its shortest form, or that runs past ten bytes.
    Varint,
    /// Bytes after the message.
    Trailing,
    /// An algorithm this release does not read.
    Algorithm(u8),
    /// Ranges that are not in order, not apart, or empty.
    Ranges,
    /// A range whose end does not fit in a `u64`.
    Overflow,
    /// A signature list whose lengths are not the length of the content it claims to be about.
    Length,
    /// A signature list cut with a different average from the one the candidate was cut with.
    Parameters,
    /// A plan that copies past the end of the candidate, which no honest list produces.
    Candidate,
    /// A plan that does not cover the target exactly once.
    Plan,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Average => {
                f.write_str("the average chunk size is not a power of two from 256 bytes to 4 MiB")
            }
            Self::ChunkLength => f.write_str("a chunk length is zero or past the largest chunk"),
            Self::TooManyEntries => f.write_str("more entries than one message may carry"),
            Self::Truncated => f.write_str("the message ends early"),
            Self::Varint => {
                f.write_str("a varint is not in its shortest form or runs past ten bytes")
            }
            Self::Trailing => f.write_str("bytes follow the message"),
            Self::Algorithm(algo) => write!(f, "algorithm {algo} is not one this release reads"),
            Self::Ranges => f.write_str("ranges are empty, not ascending, overlap, or touch"),
            Self::Overflow => f.write_str("a range ends past the largest offset"),
            Self::Length => f.write_str("the chunk lengths are not the length of the content"),
            Self::Parameters => f.write_str(
                "the signatures and the candidate were cut with different average sizes",
            ),
            Self::Candidate => f.write_str("a matched chunk runs past the end of the candidate"),
            Self::Plan => f.write_str("the plan does not cover the target once"),
        }
    }
}

impl std::error::Error for Error {}
