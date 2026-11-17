//! The BIP-0039 English word list (MIT), 2048 words, included from `words.txt`.
//!
//! The file's SHA-256 is `2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda`; the
//! test below checks it, so a changed list is a changed vector, not a silent edit.

use std::sync::LazyLock;

pub const TEXT: &str = include_str!("../words.txt");

pub const SHA256: &str = "2f5eed53a4727b4bf8880d8f3f199efc90e58503646d9ff8eff3a2ed3b24dbda";

/// One word per index, 0..=2047, in the published order.
pub static WORDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| TEXT.lines().collect::<Vec<_>>());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_the_pinned_one() {
        assert_eq!(WORDS.len(), 2048);
        assert!(WORDS.windows(2).all(|w| w[0] < w[1]), "BIP-0039 is sorted");
        let digest = ring::digest::digest(&ring::digest::SHA256, TEXT.as_bytes());
        let hex: String = digest.as_ref().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, SHA256);
    }
}
