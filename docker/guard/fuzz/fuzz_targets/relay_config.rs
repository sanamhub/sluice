#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_relay_guard::{Mode, check_config, open_config};

// Any bytes as the text of a config, in each mode the guard has, with and without
// IROH_RELAY_ACCESS_TOKEN. The check never panics, and every refusal is one line that holds no
// control character and names an unknown key only by a name of 1 to 64 letters, digits, `_` or
// `-`. The same bytes as a file name, in a folder that does not exist, give a refusal that holds
// no control character either.
fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
        for token in [None, Some("t")] {
            if let Err(refusal) = check_config(&text, mode, token) {
                one_line(&refusal.to_string());
                named_plainly(&refusal.to_string());
            }
        }
    }
    // No folder by this name exists, so the open fails at the first part and reads nothing.
    let path = format!("/sluice-relay-guard-fuzz-no-such-folder/{text}");
    if let Err(refusal) = open_config(std::path::Path::new(&path)) {
        one_line(&refusal.to_string());
    }
});

fn one_line(message: &str) {
    assert!(!message.chars().any(char::is_control), "{message:?}");
}

// Only for a config: a path is printed whole, escaped, and may itself say "unknown key".
fn named_plainly(message: &str) {
    if let Some(rest) = message.split("unknown key `").nth(1) {
        let name = rest.split('`').next().unwrap_or_default();
        let plain = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-');
        assert!(
            (1..=64).contains(&name.len()) && name.bytes().all(plain),
            "{message:?}"
        );
    }
}
