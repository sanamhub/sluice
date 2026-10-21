#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::manifest::{Limits, Manifest};

// Small limits keep one input fast, and put the limit rules in reach of short inputs.
const LIMITS: Limits = Limits {
    max_bytes: 1 << 16,
    max_entries: 1000,
};

// Whatever decodes must have exactly one spelling: the encoder writes the same bytes back, and
// those bytes decode to the same manifest. A second spelling of a manifest would be two roots for
// one tree, and a way to get past a list of the manifests that were refused.
fuzz_target!(|data: &[u8]| {
    let Ok(manifest) = Manifest::decode(data, LIMITS) else {
        return;
    };
    let encoded = manifest.encode();
    assert_eq!(encoded, data, "second spelling");
    assert!(matches!(Manifest::decode(&encoded, LIMITS), Ok(again) if again == manifest));
});
