//! The code as a person reads it: `<nameplate>-<word>-<word>`.
//!
//! The nameplate is public: the relay hands it out and both peers name it. The two words are the
//! secret. [`Code`] therefore has no `Display`, no `Serialize` and no accessor that returns a word
//! string; [`Code::text`] is the one explicit spelling, and [`Code::password`] is the SPAKE2 input.

use std::fmt;

use zeroize::Zeroizing;

use crate::words;

/// The largest nameplate the protocol allows.
pub const MAX_NAMEPLATE: u32 = 9999;

/// How many words a code carries.
pub const WORDS_PER_CODE: usize = 2;

/// A parsed code: the nameplate and two indices into the BIP-0039 English list.
///
/// Equality compares the nameplate and the two indices. The type is secret material, so its `Debug`
/// shows the nameplate and hides the indices; there is no `Display`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Code {
    nameplate: u32,
    words: [u16; WORDS_PER_CODE],
}

/// Why a code was refused. The message names the rule, never the code or a part of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CodeError {
    /// Not three parts separated by two `-` characters.
    #[error("a code has three parts separated by two '-' characters")]
    Shape,
    /// Empty, longer than four digits, a leading zero, `0`, or above 9999.
    #[error("the nameplate must be a decimal number from 1 to 9999 with no leading zero")]
    Nameplate,
    /// One of the words is not on the BIP-0039 English list, or a part is not letters.
    #[error("a word is not on the BIP-0039 English list")]
    Word,
}

impl Code {
    /// Builds a code from a nameplate and two word indices.
    ///
    /// # Errors
    ///
    /// [`CodeError::Nameplate`] when the nameplate is outside `1..=9999`, and [`CodeError::Word`]
    /// when an index is past the list.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_meet::Code;
    ///
    /// let code = Code::new(7, [1247, 1936])?;
    /// assert_eq!(code.text(), "7-orbit-velvet");
    /// # Ok::<(), sluice_meet::CodeError>(())
    /// ```
    pub fn new(nameplate: u32, words: [u16; WORDS_PER_CODE]) -> Result<Self, CodeError> {
        if !(1..=MAX_NAMEPLATE).contains(&nameplate) {
            return Err(CodeError::Nameplate);
        }
        if words.iter().any(|index| words::word(*index).is_none()) {
            return Err(CodeError::Word);
        }
        Ok(Self { nameplate, words })
    }

    /// Parses a code exactly as `SPEC.md` section 4.1.1 says: three parts, two `-`, a nameplate
    /// without a leading zero, and two words, with case ignored.
    ///
    /// # Errors
    ///
    /// The [`CodeError`] names the rule that failed. Its message never holds the text.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_meet::Code;
    ///
    /// assert_eq!(Code::parse("7-ORBIT-Velvet")?.text(), "7-orbit-velvet");
    /// assert!(Code::parse("07-orbit-velvet").is_err());
    /// # Ok::<(), sluice_meet::CodeError>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self, CodeError> {
        let mut parts = text.split('-');
        let plate = parts.next().ok_or(CodeError::Shape)?;
        let first = parts.next().ok_or(CodeError::Shape)?;
        let second = parts.next().ok_or(CodeError::Shape)?;
        if parts.next().is_some() {
            return Err(CodeError::Shape);
        }
        let nameplate = parse_nameplate(plate)?;
        let words = [
            words::index(first).ok_or(CodeError::Word)?,
            words::index(second).ok_or(CodeError::Word)?,
        ];
        Self::new(nameplate, words)
    }

    /// The nameplate, which is public.
    pub fn nameplate(&self) -> u32 {
        self.nameplate
    }

    /// The two word indices, in the order the code spells them. A vector runner needs them; a
    /// caller that shows a code uses [`Code::text`].
    pub fn word_indices(&self) -> [u16; WORDS_PER_CODE] {
        self.words
    }

    /// The one lower-case spelling an encoder writes.
    ///
    /// This is explicit because the type has no `Display`: a code reaches a terminal and a log by a
    /// call someone wrote, not by an argument to `format!` or `tracing`.
    pub fn text(&self) -> String {
        format!(
            "{}-{}-{}",
            self.nameplate,
            words::word(self.words[0]).unwrap_or_default(),
            words::word(self.words[1]).unwrap_or_default()
        )
    }

    /// The SPAKE2 password: the two words joined by one `-`, exactly as the code spells them. The
    /// buffer is wiped when it drops.
    pub fn password(&self) -> Zeroizing<String> {
        Zeroizing::new(format!(
            "{}-{}",
            words::word(self.words[0]).unwrap_or_default(),
            words::word(self.words[1]).unwrap_or_default()
        ))
    }
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Code")
            .field("nameplate", &self.nameplate)
            .field("words", &format_args!("<redacted>"))
            .finish()
    }
}

fn parse_nameplate(text: &str) -> Result<u32, CodeError> {
    if text.is_empty()
        || text.len() > 4
        || !text.bytes().all(|byte| byte.is_ascii_digit())
        || (text.len() > 1 && text.starts_with('0'))
    {
        return Err(CodeError::Nameplate);
    }
    let value: u32 = text.parse().map_err(|_| CodeError::Nameplate)?;
    if !(1..=MAX_NAMEPLATE).contains(&value) {
        return Err(CodeError::Nameplate);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_round_trips_in_lower_case() {
        let code = Code::parse("7-orbit-velvet").unwrap();
        assert_eq!(code.nameplate(), 7);
        assert_eq!(code.word_indices(), [1247, 1936]);
        assert_eq!(code.text(), "7-orbit-velvet");
        assert_eq!(code.password().as_str(), "orbit-velvet");
    }

    #[test]
    fn case_is_ignored_on_input_and_lower_case_on_output() {
        for text in ["7-ORBIT-Velvet", "7-OrBiT-vElVeT"] {
            assert_eq!(Code::parse(text).unwrap().text(), "7-orbit-velvet");
        }
    }

    #[test]
    fn the_nameplate_rules_are_each_refused() {
        for text in [
            "0-orbit-velvet",
            "00-orbit-velvet",
            "07-orbit-velvet",
            "10000-orbit-velvet",
            "12345-orbit-velvet",
            "+7-orbit-velvet",
            "seven-orbit-velvet",
            "-orbit-velvet",
        ] {
            assert_eq!(Code::parse(text), Err(CodeError::Nameplate), "{text:?}");
        }
    }

    #[test]
    fn the_shape_rules_are_each_refused() {
        for text in [
            "",
            "7",
            "7-orbit",
            "7-orbit-velvet-extra",
            "7--velvet",
            "--",
            "7-orbit-",
        ] {
            assert!(
                matches!(
                    Code::parse(text),
                    Err(CodeError::Shape | CodeError::Nameplate | CodeError::Word)
                ),
                "{text:?} parsed"
            );
        }
    }

    #[test]
    fn a_word_not_on_the_list_is_refused() {
        assert_eq!(Code::parse("7-orbit-notaword"), Err(CodeError::Word));
        assert_eq!(Code::parse("7-orbit-velvet!"), Err(CodeError::Word));
        assert_eq!(Code::parse("7-orbit-vel vet"), Err(CodeError::Word));
    }

    #[test]
    fn the_error_and_debug_never_hold_the_code() {
        for text in ["7-orbit-velvet!", "7-orbit-notaword", "07-orbit-velvet"] {
            if let Err(error) = Code::parse(text) {
                let shown = error.to_string();
                assert!(!shown.contains("orbit"), "{shown}");
                assert!(!shown.contains("velvet"), "{shown}");
                assert!(!shown.contains('7'), "{shown}");
            }
        }
        let shown = format!("{:?}", Code::parse("7-orbit-velvet").unwrap());
        assert!(shown.contains("7"), "{shown}");
        assert!(!shown.contains("orbit"), "{shown}");
        assert!(!shown.contains("velvet"), "{shown}");
        assert!(!shown.contains("1247"), "{shown}");
    }

    #[test]
    fn new_applies_the_same_rules_as_parse() {
        assert_eq!(Code::new(0, [0, 0]), Err(CodeError::Nameplate));
        assert_eq!(Code::new(10_000, [0, 0]), Err(CodeError::Nameplate));
        assert_eq!(Code::new(7, [0, 2048]), Err(CodeError::Word));
        assert_eq!(Code::new(7, [0, 2047]).unwrap().text(), "7-abandon-zoo");
    }
}
