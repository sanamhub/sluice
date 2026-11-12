//! A plan is only ever a hint, so every test here is about a hint that saves bytes, a hint that is
//! wrong, and the one promise that matters: applying a plan to the content and the candidate gives
//! the content.

use proptest::prelude::*;

use super::{Candidate, Span, plan};
use crate::{Entry, Error, Params, SigList};

/// The signatures a provider would send for `data`.
fn signatures(params: &Params, data: &[u8]) -> SigList {
    let entries: Vec<Entry> = params
        .chunks(data)
        .map(|chunk| Entry::of_bytes(chunk).unwrap())
        .collect();
    SigList::new(params.avg(), entries).unwrap()
}

/// Deterministic bytes, so a failure is the same failure on the next run.
fn content(len: usize, seed: u8) -> Vec<u8> {
    let mut state = u64::from(seed) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

fn params() -> Params {
    Params::default()
}

/// A candidate cut with one average and signatures cut with another are refused: nothing would
/// match, and a caller has to know its hint is not about this candidate rather than pay for every
/// byte.
#[test]
fn signatures_for_another_average_are_refused() {
    let data = content(70_000, 2);
    let candidate = Candidate::new(&params(), &data).unwrap();
    let other = Params::new(512).unwrap();
    assert!(plan(&signatures(&params(), &data), &candidate, data.len() as u64).is_ok());
    assert_eq!(
        plan(&signatures(&other, &data), &candidate, data.len() as u64),
        Err(Error::Parameters)
    );
}

/// A requester that already has the content fetches nothing.
#[test]
fn an_identical_candidate_is_entirely_local() {
    let data = content(70_000, 1);
    let candidate = Candidate::new(&params(), &data).unwrap();
    let plan = plan(&signatures(&params(), &data), &candidate, data.len() as u64).unwrap();
    assert_eq!(plan.fetch_bytes(), 0);
    assert_eq!(plan.local_bytes(), data.len() as u64);
    assert_eq!(plan.rebuild(&data, &data).unwrap(), data);
}

/// One changed chunk is the only one fetched: the plan is what turns a changed file into a few
/// ranges.
#[test]
fn a_changed_chunk_is_the_only_one_fetched() {
    let data = content(70_000, 2);
    let mut changed = data.clone();
    changed[30_000] ^= 0xff;
    let candidate = Candidate::new(&params(), &changed).unwrap();
    let plan = plan(&signatures(&params(), &data), &candidate, data.len() as u64).unwrap();
    assert!(plan.fetch_bytes() > 0, "nothing was fetched");
    assert!(
        plan.fetch_bytes() < 70_000 / 4,
        "{} of 70000 bytes were fetched",
        plan.fetch_bytes()
    );
    assert_eq!(plan.rebuild(&data, &changed).unwrap(), data);
}

/// Nothing in common is everything fetched, and the plan still covers the target once.
#[test]
fn unrelated_content_is_all_fetched() {
    let data = content(70_000, 3);
    let other = content(70_000, 4);
    let candidate = Candidate::new(&params(), &other).unwrap();
    let plan = plan(&signatures(&params(), &data), &candidate, data.len() as u64).unwrap();
    assert_eq!(plan.local_bytes(), 0);
    assert_eq!(plan.fetch_bytes(), data.len() as u64);
    assert_eq!(plan.rebuild(&data, &other).unwrap(), data);
}

/// Content inserted at the front moves every offset and changes no chunk after it, so almost
/// everything still comes from the candidate.
#[test]
fn content_inserted_at_the_front_leaves_the_rest_local() {
    let old = content(70_000, 5);
    let lead = content(5_000, 6);
    let mut new = lead.clone();
    new.extend_from_slice(&old);
    let candidate = Candidate::new(&params(), &old).unwrap();
    let plan = plan(&signatures(&params(), &new), &candidate, new.len() as u64).unwrap();
    assert!(
        plan.fetch_bytes() < 3 * params().max() as u64,
        "{} bytes were fetched for a 5000-byte insert",
        plan.fetch_bytes()
    );
    assert_eq!(plan.rebuild(&new, &old).unwrap(), new);
}

/// A list cut from other content is refused before anything is planned.
#[test]
fn a_list_for_other_content_is_refused() {
    let data = content(70_000, 7);
    let candidate = Candidate::new(&params(), &data).unwrap();
    let signatures = signatures(&params(), &data);
    assert_eq!(
        plan(&signatures, &candidate, data.len() as u64 + 1).unwrap_err(),
        Error::Length
    );
}

/// A list whose entries do not describe the candidate's chunks matches nothing, and everything is
/// fetched: the matcher trusts a list only for the lengths and hashes it asks about, and a match is
/// taken from the candidate's own chunks.
#[test]
fn a_list_that_does_not_describe_the_candidate_matches_nothing() {
    let data = content(20_000, 8);
    let candidate = Candidate::new(&params(), &data).unwrap();
    let strong = *Entry::of_bytes(&data).unwrap().strong();
    let forged = SigList::new(
        params().avg(),
        vec![Entry::new(u32::try_from(data.len()).unwrap(), strong).unwrap()],
    )
    .unwrap();
    let plan = plan(&forged, &candidate, data.len() as u64).unwrap();
    assert_eq!(plan.local_bytes(), 0);
    assert_eq!(plan.fetch_bytes(), data.len() as u64);
}
/// Adjacent matches are one run, so a plan asks for one range instead of one per chunk.
#[test]
fn adjacent_matches_are_one_span() {
    let data = content(70_000, 9);
    let candidate = Candidate::new(&params(), &data).unwrap();
    let plan = plan(&signatures(&params(), &data), &candidate, data.len() as u64).unwrap();
    assert_eq!(plan.spans().len(), 1, "{:?}", plan.spans());
    assert_eq!(plan.need_ranges([9; 32]).unwrap().wanted(), 0);
}

/// The ranges a plan asks for are ascending and apart, so they are a message the wire accepts.
#[test]
fn the_ranges_to_fetch_are_a_valid_message() {
    let data = content(70_000, 10);
    let mut changed = data.clone();
    let params = params();
    let step = params.avg() as usize;
    for at in (step..changed.len()).step_by(step * 2) {
        changed[at] ^= 0xff;
    }
    let candidate = Candidate::new(&params, &changed).unwrap();
    let plan = plan(&signatures(&params, &data), &candidate, data.len() as u64).unwrap();
    let need = plan.need_ranges([7; 32]).unwrap();
    assert_eq!(need.wanted(), plan.fetch_bytes());
    assert!(
        need.ranges()
            .windows(2)
            .all(|pair| pair[0].end() <= pair[1].start())
    );
}

proptest! {
    /// The promise the engine relies on: a plan applied to the content and the candidate gives the
    /// content, whatever the two files are.
    #[test]
    fn a_plan_rebuilds_the_target(
        target in prop::collection::vec(any::<u8>(), 0..30_000),
        candidate_bytes in prop::collection::vec(any::<u8>(), 0..30_000),
    ) {
        let candidate = Candidate::new(&params(), &candidate_bytes).unwrap();
        let signatures = signatures(&params(), &target);
        let plan = plan(&signatures, &candidate, target.len() as u64).unwrap();
        prop_assert_eq!(plan.local_bytes() + plan.fetch_bytes(), target.len() as u64);
        prop_assert_eq!(plan.rebuild(&target, &candidate_bytes).unwrap(), target);
    }

    /// No plan, however hostile the list, names a candidate range outside the candidate.
    #[test]
    fn a_plan_never_reads_outside_the_candidate(
        target in prop::collection::vec(any::<u8>(), 0..20_000),
        candidate_bytes in prop::collection::vec(any::<u8>(), 0..20_000),
    ) {
        let candidate = Candidate::new(&params(), &candidate_bytes).unwrap();
        let signatures = signatures(&params(), &target);
        let plan = plan(&signatures, &candidate, target.len() as u64).unwrap();
        for span in plan.spans() {
            if let Span::Local { target, candidate: from } = span {
                prop_assert!(from + target.len() <= candidate_bytes.len() as u64);
            }
        }
    }
}

/// A candidate built from entries cut in blocks is the candidate built from the whole data: a plan
/// made with either is the same plan.
#[test]
fn a_candidate_from_entries_plans_like_one_from_the_bytes() {
    let params = params();
    let target = content(200_000, 4);
    let mut candidate_bytes = target.clone();
    candidate_bytes[90_000] ^= 1;
    let from_bytes = Candidate::new(&params, &candidate_bytes).unwrap();
    let mut entries = Vec::new();
    let mut cut = crate::StreamCut::new(params);
    let mut each = |chunk: &[u8]| entries.push(Entry::of_bytes(chunk).unwrap());
    for block in candidate_bytes.chunks(10_000) {
        cut.feed(block, &mut each);
    }
    cut.finish(&mut each);
    let from_entries = Candidate::from_entries(&params, entries);
    assert_eq!(from_entries.len(), from_bytes.len());
    let list = signatures(&params, &target);
    let len = target.len() as u64;
    assert_eq!(
        plan(&list, &from_entries, len).unwrap(),
        plan(&list, &from_bytes, len).unwrap()
    );
}
