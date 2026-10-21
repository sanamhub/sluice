#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::manifest::pack::Unpacker;
use sluice_core::manifest::{Limits, Loc, Manifest};

// Small limits keep one input fast, and put the limit rules in reach of short inputs.
const LIMITS: Limits = Limits {
    max_bytes: 1 << 16,
    max_entries: 1000,
};

/// A pack is fed to the unpacker in full up to this size. A bigger one, which a manifest can
/// declare in a few bytes, is fed this much and dropped, so one input stays fast.
const FULL: u64 = 1 << 18;

// The input is a manifest, or a manifest followed by piece sizes for the unpacker and one byte
// that says how many there are. Whatever decodes must have exactly one spelling: the encoder
// writes the same bytes back, and those bytes decode to the same manifest. A second spelling of a
// manifest would be two roots for one tree, and a way to get past a list of the manifests that
// were refused. Then every pack of it is split by the unpacker in pieces of those sizes.
fuzz_target!(|data: &[u8]| {
    let (bytes, cuts) = match Manifest::decode(data, LIMITS) {
        Ok(_) => (data, &[][..]),
        Err(_) => split(data),
    };
    let Ok(manifest) = Manifest::decode(bytes, LIMITS) else {
        return;
    };
    let encoded = manifest.encode();
    assert_eq!(encoded, bytes, "second spelling");
    assert!(matches!(Manifest::decode(&encoded, LIMITS), Ok(again) if again == manifest));
    for index in 0..manifest.packs().len() {
        unpack(&manifest, index as u32, cuts);
    }
});

/// The manifest and the piece sizes of an input whose last byte counts the piece sizes before it.
fn split(data: &[u8]) -> (&[u8], &[u8]) {
    let Some((&count, rest)) = data.split_last() else {
        return (data, &[]);
    };
    let count = usize::from(count).min(rest.len());
    rest.split_at(rest.len() - count)
}

/// Feeds pack `index` in pieces whose sizes cycle through `cuts`, and checks that the runs cover
/// every byte once, in order, and that each file gets exactly its length and ends once.
fn unpack(manifest: &Manifest, index: u32, cuts: &[u8]) {
    let size = manifest.packs()[index as usize].size;
    let mut unpacker = Unpacker::new(manifest, index).expect("the pack exists");
    let zeros = [0u8; 4096];
    let mut got = vec![0u64; manifest.entries().len()];
    let mut ended = vec![false; manifest.entries().len()];
    let mut last_entry = 0;
    let (mut fed, mut turn, mut empty_before) = (0u64, 0usize, false);
    let target = size.min(FULL);
    while fed < target {
        let cut = cuts.get(turn % cuts.len().max(1)).map_or(1000, |&c| usize::from(c) * 17);
        turn += 1;
        // An empty piece is allowed, and the one after it is not, so the loop always moves on.
        let want = if empty_before { cut.max(1) } else { cut };
        let take = (want as u64).min(target - fed).min(zeros.len() as u64) as usize;
        empty_before = take == 0;
        let runs = unpacker.feed(&zeros[..take]).expect("within the pack");
        let mut at = 0;
        for run in runs {
            assert_eq!(run.bytes.start, at, "a gap or an overlap in a piece");
            assert!(run.entry >= last_entry && !ended[run.entry], "out of order or ended twice");
            last_entry = run.entry;
            at = run.bytes.end;
            got[run.entry] += run.bytes.len() as u64;
            ended[run.entry] = run.ends;
        }
        assert_eq!(at, take, "a piece not covered to its end");
        fed += take as u64;
    }
    if size > FULL {
        return;
    }
    for run in unpacker.finish().expect("the whole pack was fed") {
        assert!(run.bytes.is_empty() && run.ends && !ended[run.entry]);
        ended[run.entry] = true;
    }
    for (at, entry) in manifest.entries().iter().enumerate() {
        if let Loc::Packed { pack_index, len, .. } = entry.loc {
            if pack_index == index {
                assert!(ended[at] && got[at] == len, "a file did not get its bytes");
            }
        }
    }
    let mut over = Unpacker::new(manifest, index).expect("the pack exists");
    if size < zeros.len() as u64 {
        assert!(over.feed(&zeros[..size as usize + 1]).is_err(), "a byte past the pack");
    }
}
