#![no_main]

use libfuzzer_sys::fuzz_target;
use sluice_core::engine::{decode_hello, encode_hello};

// The greeting is three varints with no length prefix. Whatever decodes must have exactly one
// spelling, which is also what the encoder writes, so two greetings can never name one exchange.
fuzz_target!(|data: &[u8]| {
    let Ok(hello) = decode_hello(data) else {
        return;
    };
    let encoded = encode_hello(hello);
    assert_eq!(encoded, data, "second spelling");
    assert_eq!(decode_hello(&encoded), Ok(hello));
});