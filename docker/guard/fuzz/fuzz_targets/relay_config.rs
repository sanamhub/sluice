#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_relay_guard::{Mode, check_config};

// Any bytes as the text of a config, in each mode the guard has. The check never panics, and every
// refusal is one line.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
        if let Err(refusal) = check_config(&text, mode) {
            assert!(!refusal.to_string().contains('\n'));
        }
    }
});
