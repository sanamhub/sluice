#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_delta::{MAX_ENTRIES, Params, SigList};

// The input is the bytes of a signature list as a peer could send them. A decoder that accepts bytes
// must accept exactly one spelling of them: the list it read encodes back to the same bytes, its
// average is one a cutter takes, no entry is longer than that average's largest chunk, there are no
// more entries than a message may carry, and the lengths add up to the sum it reports. A decoder
// that refuses must do so without panicking and without reserving what the count said.
fuzz_target!(|data: &[u8]| {
    let Ok(list) = SigList::decode(data) else {
        return;
    };
    assert_eq!(list.encode(), data, "one list, one spelling");
    let params = Params::new(list.avg_bytes()).expect("a decoded list has a valid average");
    assert!(list.entries().len() <= MAX_ENTRIES as usize);
    let mut total = 0u64;
    for entry in list.entries() {
        assert!(entry.len() >= 1 && entry.len() <= params.max());
        total += u64::from(entry.len());
    }
    assert_eq!(list.total_len(), total);
});
