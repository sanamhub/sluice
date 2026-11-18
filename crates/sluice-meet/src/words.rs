//! The BIP-0039 English word list, 2048 words in the published order.
//!
//! The file is the reference copy from `SPEC.md` section 4.1.5. Its bytes with one LF after every
//! word have SHA-256 `2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda`; the test
//! below checks the digest, the count and the order, so a changed list is a changed vector and not a
//! silent edit. The list is MIT licensed; `WORDS-LICENSE` carries the notice.

use std::sync::LazyLock;

/// Every word, one LF after each, exactly as the pinned file holds it.
pub const TEXT: &str = include_str!("../words.txt");

/// The SHA-256 of [`TEXT`], from `SPEC.md` section 4.1.1.
pub const SHA256: &str = "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda";

/// The list in its published order: index `i` is the word whose 11-bit value is `i`.
pub static WORDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| TEXT.lines().collect());

/// The word at `index`, or `None` past the list.
///
/// # Examples
///
/// ```
/// use sluice_meet::words;
///
/// assert_eq!(words::word(0), Some("abandon"));
/// assert_eq!(words::word(2047), Some("zoo"));
/// assert_eq!(words::word(2048), None);
/// ```
pub fn word(index: u16) -> Option<&'static str> {
    WORDS.get(usize::from(index)).copied()
}

/// The index of `word`, ignoring ASCII case, or `None` when it is not on the list.
///
/// # Examples
///
/// ```
/// use sluice_meet::words;
///
/// assert_eq!(words::index("Velvet"), Some(1936));
/// assert_eq!(words::index("notaword"), None);
/// ```
pub fn index(word: &str) -> Option<u16> {
    let lower = word.to_ascii_lowercase();
    WORDS
        .binary_search(&lower.as_str())
        .ok()
        .and_then(|index| u16::try_from(index).ok())
}

/// The SHA-256 of [`TEXT`], as lower-case hex.
pub fn digest_hex() -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, TEXT.as_bytes());
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_the_pinned_one() {
        assert_eq!(WORDS.len(), 2048);
        assert!(WORDS.windows(2).all(|pair| pair[0] < pair[1]), "sorted");
        assert_eq!(digest_hex(), SHA256);
        assert_eq!(TEXT.lines().count(), 2048);
    }

    #[test]
    fn every_word_is_lower_case_ascii() {
        for word in WORDS.iter() {
            assert!(
                word.bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'\''),
                "{word:?}"
            );
        }
    }
}
