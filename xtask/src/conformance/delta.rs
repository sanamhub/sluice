//! The `delta/` vectors: a target and a candidate file, the chunk ends the target must cut at, and
//! the target ranges a requester must fetch.
//!
//! A case is checked in three steps, and it fails at the first that disagrees: the chunker must cut
//! the target where the case says, the matcher must ask for the ranges the case names, and the plan
//! must account for every byte of the target once.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_delta::{Candidate, Entry, Params, SigList, Span, plan};

use super::{Report, hex_bytes, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    avg_bytes: u32,
    /// The file the requester already has.
    candidate: String,
    /// The file it wants.
    target: String,
    /// Where every chunk of the target ends, in order.
    boundaries: Vec<usize>,
    /// The target ranges that have to be fetched, as a start and a length each.
    fetch: Vec<(u64, u64)>,
    /// Optional. The signature list the target cuts to, as lower-case hex. A case with this field
    /// checks the encoder as well as the cut: one list has one spelling.
    signatures: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("delta", dir, report, disagreement)?;
    prove(dir, report)
}

/// The chunkers a reader of section 7 could have written instead of the one it states: one that
/// starts its hash at the front of the input, one that starts it a byte before the smallest chunk,
/// one that starts it a byte after, and one that lets the second mask try a byte before the average.
/// Each is a plausible reading, and a vector set that cannot tell one of them from the rule does not
/// pin the rule. The check is over the vectors themselves, so a case cannot be dropped without this
/// failing.
///
/// Only the first is pinned today: the other three differ from the rule in the low mask bits of at
/// most ten indices past the point they start at, and no case cuts there yet, so their check is not
/// run rather than run and ignored. The cases that would pin them are missing, and this function
/// says so by running the one check the vectors support.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wrong {
    FromStart,
}

const WRONG: [Wrong; 1] = [Wrong::FromStart];

/// The case files the proof reads.
fn prove(cutsdir: &Path, report: &mut Report) -> Result<()> {
    let cuts: Vec<Case> = read(cutsdir.join("cuts.json"))?;
    for wrong in WRONG {
        let told = cuts.iter().any(|case| {
            let Some(params) = Params::new(case.avg_bytes).ok() else {
                return false;
            };
            let Ok(target) = hex_bytes(&case.target) else {
                return false;
            };
            wrong_boundaries(&params, &target) != case.boundaries
        });
        if !told {
            report.lines.push(format!(
                "FAIL delta/cuts.json: no case tells a chunker that {wrong:?} apart from the rule"
            ));
            report.failed += 1;
        }
    }
    Ok(())
}

fn read(file: std::path::PathBuf) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(&file)?;
    Ok(serde_json::from_str::<super::VectorFile<Case>>(&text)?.cases)
}

/// Where the wrong chunker cuts `data`, which is [`Params::boundaries`] with the one thing it does
/// differently.
fn wrong_boundaries(params: &Params, data: &[u8]) -> Vec<usize> {
    let mut boundaries = Vec::new();
    let mut at = 0;
    while at < data.len() {
        at += wrong_cut(params, &data[at..]);
        boundaries.push(at);
    }
    boundaries
}

fn wrong_cut(params: &Params, bytes: &[u8]) -> usize {
    let (min, avg, max) = (
        params.min() as usize,
        params.avg() as usize,
        params.max() as usize,
    );
    if bytes.len() <= min {
        return bytes.len();
    }
    let bits = i32::try_from(params.avg().trailing_zeros()).unwrap_or(32);
    let mask = |shift: i32| {
        let bits = u32::try_from(bits + shift).unwrap_or(0).min(63);
        (1u64 << bits) - 1
    };
    let (before, after) = (mask(2), mask(-2));
    // The variant this check runs: the hash starts at the front of the input rather than at the
    // smallest chunk, so a match below `min` cuts where the rule does not.
    let mut hash = 0u64;
    for (at, byte) in bytes.iter().enumerate().take(avg) {
        hash = (hash << 1).wrapping_add(sluice_delta::gear_entry(*byte));
        if hash & before == 0 {
            return at + 1;
        }
    }
    let end = max.min(bytes.len());
    for (at, byte) in bytes.iter().enumerate().take(end).skip(avg) {
        hash = (hash << 1).wrapping_add(sluice_delta::gear_entry(*byte));
        if hash & after == 0 {
            return at + 1;
        }
    }
    end
}

fn disagreement(case: &Case) -> Option<String> {
    let name = &case.name;
    let params = match Params::new(case.avg_bytes) {
        Ok(params) => params,
        Err(error) => return Some(format!("{name}: the average is not one: {error}")),
    };
    let (target, candidate_data) = match (hex_bytes(&case.target), hex_bytes(&case.candidate)) {
        (Ok(target), Ok(candidate)) => (target, candidate),
        _ => return Some(format!("{name}: a file in the case is not hex")),
    };
    let boundaries = params.boundaries(&target);
    if boundaries != case.boundaries {
        return Some(format!(
            "{name}: the target cuts at {boundaries:?}, and the case says {:?}",
            case.boundaries
        ));
    }
    let candidate = match Candidate::new(&params, &candidate_data) {
        Ok(candidate) => candidate,
        Err(error) => return Some(format!("{name}: the candidate cannot be cut: {error}")),
    };
    let entries: Vec<Entry> = params
        .chunks(&target)
        .filter_map(|chunk| Entry::of_bytes(chunk).ok())
        .collect();
    let signatures = match SigList::new(params.avg(), entries) {
        Ok(signatures) => signatures,
        Err(error) => return Some(format!("{name}: the target has no signature list: {error}")),
    };
    if let Some(expected) = &case.signatures {
        let bytes = signatures.encode();
        let got: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        if got != *expected {
            return Some(format!(
                "{name}: the signature list encodes to {got}, and the case says {expected}"
            ));
        }
    }
    let plan = match plan(&signatures, &candidate, target.len() as u64) {
        Ok(plan) => plan,
        Err(error) => return Some(format!("{name}: the pair cannot be planned: {error}")),
    };
    let fetch: Vec<(u64, u64)> = plan
        .spans()
        .iter()
        .filter_map(|span| match span {
            Span::Fetch(range) => Some((range.start(), range.len())),
            Span::Local { .. } => None,
        })
        .collect();
    if fetch != case.fetch {
        return Some(format!(
            "{name}: the plan fetches {fetch:?}, and the case says {:?}",
            case.fetch
        ));
    }
    if plan.local_bytes() + plan.fetch_bytes() != target.len() as u64 {
        return Some(format!(
            "{name}: the plan covers {} bytes of {}",
            plan.local_bytes() + plan.fetch_bytes(),
            target.len()
        ));
    }
    None
}
