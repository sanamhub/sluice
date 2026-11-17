//! The code as a person sees it: `<nameplate>-<word>-<word>`.

use crate::words::WORDS;

/// The nameplate is a decimal number from 1 to 9999, written without leading zeros.
pub const MAX_NAMEPLATE: u32 = 9999;

/// A parsed code. The words are indices into the BIP-0039 list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Code {
    pub nameplate: u32,
    pub words: [u16; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeError {
    Shape,
    Nameplate,
    Word,
}

impl Code {
    pub fn new(nameplate: u32, words: [u16; 2]) -> Result<Self, CodeError> {
        if nameplate == 0 || nameplate > MAX_NAMEPLATE {
            return Err(CodeError::Nameplate);
        }
        if words.iter().any(|&i| usize::from(i) >= WORDS.len()) {
            return Err(CodeError::Word);
        }
        Ok(Self { nameplate, words })
    }

    pub fn parse(text: &str) -> Result<Self, CodeError> {
        let mut parts = text.split('-');
        let plate = parts.next().ok_or(CodeError::Shape)?;
        let first = parts.next().ok_or(CodeError::Shape)?;
        let second = parts.next().ok_or(CodeError::Shape)?;
        if parts.next().is_some() {
            return Err(CodeError::Shape);
        }
        if plate.is_empty() || plate.len() > 4 || !plate.bytes().all(|b| b.is_ascii_digit()) {
            return Err(CodeError::Nameplate);
        }
        if plate.len() > 1 && plate.starts_with('0') {
            return Err(CodeError::Nameplate);
        }
        let nameplate: u32 = plate.parse().map_err(|_| CodeError::Nameplate)?;
        let words = [word_index(first)?, word_index(second)?];
        Self::new(nameplate, words)
    }

    pub fn as_string(&self) -> String {
        format!(
            "{}-{}-{}",
            self.nameplate,
            WORDS[usize::from(self.words[0])],
            WORDS[usize::from(self.words[1])]
        )
    }

    /// The SPAKE2 password. Both sides feed exactly these bytes.
    pub fn password(&self) -> String {
        format!(
            "{}-{}",
            WORDS[usize::from(self.words[0])],
            WORDS[usize::from(self.words[1])]
        )
    }
}

fn word_index(word: &str) -> Result<u16, CodeError> {
    // BIP-0039 is sorted, so one binary search finds the index and rejects anything else.
    WORDS
        .binary_search(&word.to_ascii_lowercase().as_str())
        .map(|i| i as u16)
        .map_err(|_| CodeError::Word)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_round_trips() {
        let code = Code::parse("7-orbit-velvet").unwrap();
        assert_eq!(code.as_string(), "7-orbit-velvet");
        assert_eq!(code.password(), "orbit-velvet");
    }

    #[test]
    fn case_and_leading_zero_are_checked() {
        assert_eq!(Code::parse("7-ORBIT-Velvet").unwrap().as_string(), "7-orbit-velvet");
        assert_eq!(Code::parse("07-orbit-velvet"), Err(CodeError::Nameplate));
        assert_eq!(Code::parse("0-orbit-velvet"), Err(CodeError::Nameplate));
        assert_eq!(Code::parse("10000-orbit-velvet"), Err(CodeError::Nameplate));
    }

    #[test]
    fn a_bad_shape_is_refused() {
        for text in ["", "7", "7-orbit", "7-orbit-velvet-x", "7--velvet", "-orbit-velvet"] {
            assert!(Code::parse(text).is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn a_word_not_on_the_list_is_refused() {
        assert_eq!(Code::parse("7-orbit-notaword"), Err(CodeError::Word));
    }
}
