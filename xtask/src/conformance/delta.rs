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
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("delta", dir, report, disagreement)
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
