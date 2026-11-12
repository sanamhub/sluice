//! The matcher: the provider's signatures and the chunks a requester already holds, in, and a plan
//! out. The plan says, for every byte of the target, whether it comes from a range of the
//! requester's own file or from the network, and it never decides that the result is right: the
//! whole file is verified against its root afterwards, so a wrong hint costs time and nothing else.

use std::collections::HashMap;

use crate::{Entry, Error, NeedRanges, Range, SigList, chunker::Params};

/// The chunks of a file the requester already has, in order, with where each one starts in it, and
/// the average they were cut at, so a plan can refuse signatures cut with another.
pub struct Candidate {
    chunks: Vec<(u64, Entry)>,
    len: u64,
    avg: u32,
}

/// An empty candidate has no chunks and no bytes, so there is no is_empty beside len.
#[allow(clippy::len_without_is_empty)]
impl Candidate {
    /// Chunks `data` with `params`, which is how a candidate is prepared. The chunks have to be cut
    /// with the parameters the provider used, or nothing matches and the plan fetches everything.
    pub fn new(params: &Params, data: &[u8]) -> Result<Self, Error> {
        let mut chunks = Vec::new();
        let mut at = 0u64;
        for chunk in params.chunks(data) {
            let entry = Entry::of_bytes(chunk)?;
            let len = u64::from(entry.len());
            chunks.push((at, entry));
            at += len;
        }
        Ok(Self {
            chunks,
            len: at,
            avg: params.avg(),
        })
    }

    /// A candidate from chunks that were cut elsewhere, in order, with `params`: how an engine
    /// prepares a file it reads in blocks and never holds whole.
    pub fn from_entries(params: &Params, entries: Vec<Entry>) -> Self {
        let mut at = 0u64;
        let mut chunks = Vec::with_capacity(entries.len());
        for entry in entries {
            let len = u64::from(entry.len());
            chunks.push((at, entry));
            at += len;
        }
        Self {
            chunks,
            len: at,
            avg: params.avg(),
        }
    }

    /// How long the candidate is.
    pub fn len(&self) -> u64 {
        self.len
    }

    /// Where each chunk of the candidate starts, by its length and its strong hash. A file can hold
    /// one chunk twice, and the first one is the one a match uses: either copy is the same bytes.
    fn index(&self) -> HashMap<(u32, [u8; 16]), u64> {
        let mut index = HashMap::with_capacity(self.chunks.len());
        for (start, entry) in &self.chunks {
            index
                .entry((entry.len(), *entry.strong()))
                .or_insert(*start);
        }
        index
    }
}

/// Where one run of the target comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Span {
    /// Bytes of the target that the requester already has, copied from `candidate` onward.
    Local {
        /// The range of the target these bytes are.
        target: Range,
        /// Where in the candidate they start.
        candidate: u64,
    },
    /// Bytes of the target that have to be fetched.
    Fetch(Range),
}

/// What a requester does to rebuild one target: the runs of bytes it copies and the runs it asks
/// for, in target order, covering every byte exactly once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    spans: Vec<Span>,
}

impl Plan {
    /// The runs, in order.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// The bytes this plan copies from the requester's own file.
    pub fn local_bytes(&self) -> u64 {
        self.runs()
            .filter(|(local, _)| *local)
            .map(|(_, range)| range.len())
            .sum()
    }

    /// The bytes this plan fetches.
    pub fn fetch_bytes(&self) -> u64 {
        self.runs()
            .filter(|(local, _)| !*local)
            .map(|(_, range)| range.len())
            .sum()
    }

    /// The target ranges to fetch, as the message that asks for them.
    pub fn need_ranges(&self, root: [u8; 32]) -> Result<NeedRanges, Error> {
        let ranges: Vec<Range> = self
            .spans
            .iter()
            .filter_map(|span| match span {
                Span::Fetch(range) => Some(*range),
                Span::Local { .. } => None,
            })
            .collect();
        NeedRanges::new(root, ranges)
    }

    /// Rebuilds the target from the bytes it fetches and the candidate it copies. `target` is the
    /// content the fetched ranges come from, which a test has and an engine has in pieces; this is
    /// the reference the engine's own assembly is checked against.
    ///
    /// The result is not verified here. It is verified against its BLAKE3 root, and that check is
    /// what makes a wrong plan harmless.
    pub fn rebuild(&self, target: &[u8], candidate: &[u8]) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(target.len());
        for span in &self.spans {
            match span {
                Span::Local {
                    target: range,
                    candidate: from,
                } => {
                    let at = usize::try_from(*from).map_err(|_| Error::Candidate)?;
                    let len = usize::try_from(range.len()).map_err(|_| Error::Candidate)?;
                    let end = at.checked_add(len).ok_or(Error::Candidate)?;
                    let bytes = candidate.get(at..end).ok_or(Error::Candidate)?;
                    out.extend_from_slice(bytes);
                }
                Span::Fetch(range) => {
                    let at = usize::try_from(range.start()).map_err(|_| Error::Plan)?;
                    let len = usize::try_from(range.len()).map_err(|_| Error::Plan)?;
                    let end = at.checked_add(len).ok_or(Error::Plan)?;
                    let bytes = target.get(at..end).ok_or(Error::Truncated)?;
                    out.extend_from_slice(bytes);
                }
            }
        }
        if out.len() != target.len() {
            return Err(Error::Plan);
        }
        Ok(out)
    }

    /// Every span as whether it is local and the target range it covers, for the sums above.
    fn runs(&self) -> impl Iterator<Item = (bool, Range)> + '_ {
        self.spans.iter().map(|span| match span {
            Span::Local { target, .. } => (true, *target),
            Span::Fetch(range) => (false, *range),
        })
    }
}

/// Matches the provider's signatures against the requester's candidate and returns the plan.
///
/// The sum of the signature lengths MUST be the target's length, because a list cut from other
/// content describes content that is not there, and the list and the candidate MUST have been cut
/// with the same average: a mismatch would fetch every byte and call it a hint. A signature that
/// matches nothing, or whose chunk would run past the end of the candidate, is fetched: a hint is a
/// peer's word, and the only thing it may cost is time.
pub fn plan(signatures: &SigList, candidate: &Candidate, target_len: u64) -> Result<Plan, Error> {
    if signatures.total_len() != target_len {
        return Err(Error::Length);
    }
    if signatures.avg_bytes() != candidate.avg {
        return Err(Error::Parameters);
    }
    let index = candidate.index();
    let mut runs = Runs::default();
    let mut at = 0u64;
    for entry in signatures.entries() {
        let target = Range::new(at, u64::from(entry.len()))?;
        at += u64::from(entry.len());
        let matched = index
            .get(&(entry.len(), *entry.strong()))
            .copied()
            .filter(|from| from.saturating_add(u64::from(entry.len())) <= candidate.len);
        match matched {
            Some(from) => runs.push(Some(from), target),
            None => runs.push(None, target),
        }
    }
    Ok(Plan {
        spans: runs.into_spans(),
    })
}

/// The runs of one plan while it is being built: adjacent spans with the same source and
/// contiguous bytes are one span, so a plan asks for one range where it would have asked for a
/// thousand.
#[derive(Default)]
struct Runs {
    runs: Vec<(Option<u64>, u64, u64)>,
}

impl Runs {
    /// Adds `target` to the runs, as local from `candidate` or as a fetch.
    fn push(&mut self, candidate: Option<u64>, target: Range) {
        let joins = match self.runs.last_mut() {
            Some((last, start, len)) => {
                *start + *len == target.start()
                    && match (last, candidate) {
                        (Some(from), Some(at)) => *from + *len == at,
                        (None, None) => true,
                        _ => false,
                    }
            }
            None => false,
        };
        if joins {
            if let Some((_, _, len)) = self.runs.last_mut() {
                *len += target.len();
            }
            return;
        }
        self.runs.push((candidate, target.start(), target.len()));
    }

    /// The runs as spans.
    fn into_spans(self) -> Vec<Span> {
        self.runs
            .into_iter()
            .map(|(candidate, start, len)| {
                let target = Range::new(start, len).expect("a run covers bytes");
                match candidate {
                    Some(candidate) => Span::Local { target, candidate },
                    None => Span::Fetch(target),
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
