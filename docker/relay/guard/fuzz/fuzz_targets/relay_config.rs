#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_relay_guard::{Mode, check_config};

// Any bytes as the text of a config, with and without --open. The check never panics, every
// refusal is one line, and a config that passes is the mode it was asked for: closed without
// --open and open with it.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for open in [false, true] {
        match check_config(&text, open) {
            Ok(mode) => assert_eq!(mode == Mode::Open, open),
            Err(refusal) => assert!(!refusal.to_string().contains('\n')),
        }
    }
});
