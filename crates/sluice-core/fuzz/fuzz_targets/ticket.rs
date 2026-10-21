#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::ticket::Ticket;

// Any text that decodes must have exactly one spelling: the encoder writes it back in lower case,
// and that text decodes to the same ticket. A second spelling of a read capability would get past a
// list of the tickets that were refused.
fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(ticket) = Ticket::decode(text) else {
        return;
    };
    let encoded = ticket.encode();
    assert_eq!(encoded, text.to_ascii_lowercase(), "second spelling");
    assert!(matches!(Ticket::decode(&encoded), Ok(again) if again == ticket));
});
