//! Crockford base32 without padding, as `SPEC.md` section 4 states it, and strict: it takes the
//! 32 characters of the alphabet in either case and nothing else.
//!
//! A forgiving decoder (Crockford's own reads `I` and `L` as `1`, `O` as `0` and skips `-`) would
//! give one ticket many spellings, and a capability with many spellings is hard to deny-list.

use crate::Error;

const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const INVALID: u8 = 0xff;

/// The value of each byte, with upper case mapped to lower. Built at compile time so the decoder
/// reads one table and never compares a character against the alphabet by hand.
const DECODE: [u8; 256] = {
    let mut table = [INVALID; 256];
    let mut i = 0;
    while i < 32 {
        table[ALPHABET[i] as usize] = i as u8;
        table[ALPHABET[i].to_ascii_uppercase() as usize] = i as u8;
        i += 1;
    }
    table
};

/// The bytes as lower-case base32, 5 bits per character, most significant bit first. The last
/// character is filled up with zero bits.
pub(super) fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let (mut acc, mut bits) = (0u32, 0u32);
    for &byte in bytes {
        acc = (acc << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(ALPHABET[((acc >> bits) & 31) as usize]));
        }
        // Only the bits not written yet matter, and keeping them alone stops the shift overflowing.
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(ALPHABET[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The bytes `text` stands for. The errors say where a character is wrong, never what it was.
pub(super) fn decode(text: &str) -> Result<Vec<u8>, Error> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for (at, byte) in text.bytes().enumerate() {
        let value = DECODE[usize::from(byte)];
        if value == INVALID {
            return Err(refuse(&format!(
                "has a character outside the alphabet at byte {at}"
            )));
        }
        acc = (acc << 5) | u32::from(value);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // 5 or more bits left over means the last character carries no data, which whole bytes cannot
    // produce.
    if bits >= 5 {
        return Err(refuse("body length cannot come from whole bytes"));
    }
    if acc != 0 {
        return Err(refuse(
            "body has non-zero unused bits in its last character",
        ));
    }
    Ok(out)
}

fn refuse(reason: &str) -> Error {
    super::refuse(reason)
}
