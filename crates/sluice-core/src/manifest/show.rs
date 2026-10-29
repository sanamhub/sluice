//! The printed form of text the CLI did not write itself: a name from a manifest or a live stream,
//! a message a peer chose. A terminal obeys an escape sequence or a bidirectional character in such
//! text, so every such character is written as an escape before the text reaches it.

/// The characters that can change what a terminal shows: the C0 and C1 controls, and the
/// bidirectional formatting characters. `SPEC.md` section 8 refuses them in a path.
pub(super) fn is_display_control(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{1f}'
            | '\u{7f}'..='\u{9f}'
            | '\u{61c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

/// The characters Unicode puts in the `Zl` and `Zp` categories. Each category holds exactly one
/// code point, and a terminal may break the line at either of them, which a name or a message from
/// a stranger could use to split the line it is printed in. They are not display controls and a
/// manifest path may hold them, so they are escaped here rather than refused there.
fn is_line_separator(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}')
}

/// `text` with every character that could change what a terminal shows written as an escape, so a
/// name or a message a stranger chose cannot reorder the line it is printed in or write an escape
/// sequence into it.
///
/// These are the characters of [`is_display_control`] and the two Unicode line separators. A
/// manifest path cannot hold any of them; a live stream's name is a file name on the sender's disk,
/// which on Unix may hold anything.
///
/// The escape for a character is `\u{…}`, so a backslash in the text could be read as the start of
/// one: a name holding the literal text `\u{202e}` would print exactly like the escape written for
/// U+202E. A backslash therefore becomes `\\` when what follows it would read as one of these
/// escapes, and only then. Doubling every backslash would make every Windows path in a message read
/// `C:\\Users\\…`, and a reader would have no way to tell which of the two backslashes the sender's
/// text held.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::path::shown;
///
/// assert_eq!(shown("a.txt"), "a.txt");
/// assert_eq!(shown("report\u{202e}fdp.exe"), r"report\u{202e}fdp.exe");
/// assert_eq!(shown("a\u{1b}[2Jb"), r"a\u{1b}[2Jb");
/// assert_eq!(shown(r"a\u{202e}b"), r"a\\u{202e}b");
/// assert_eq!(shown(r"C:\Users\me\a.txt"), r"C:\Users\me\a.txt");
/// assert_eq!(shown("a\u{2028}b\u{2029}c"), r"a\u{2028}b\u{2029}c");
/// ```
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (at, c) in text.char_indices() {
        if is_display_control(c) || is_line_separator(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else if c == '\\' && starts_an_escape(text, at) {
            out.push_str(r"\\");
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a backslash at byte `at` of `text` has to be doubled: what comes after it would read as
/// an escape this module writes. That is the literal text `u{`, and also a character that the next
/// step writes as `\u{…}`, whose backslash would otherwise join the literal one into a `\\` a reader
/// would take for the whole text.
fn starts_an_escape(text: &str, at: usize) -> bool {
    let rest = &text[at + 1..];
    match rest.chars().next() {
        None => false,
        Some('u') => rest.starts_with("u{"),
        Some(next) => is_display_control(next) || is_line_separator(next),
    }
}

#[cfg(test)]
mod tests {
    use super::shown;

    /// A name of ordinary text is left alone, whatever script it is written in.
    #[test]
    fn a_name_with_nothing_to_escape_is_unchanged() {
        for text in ["a.txt", "é/日本語.txt", "IMG_0001.JPG", ""] {
            assert_eq!(shown(text), text);
        }
    }

    /// A backslash is doubled only where what follows it would be read as an escape, so a Windows
    /// path keeps the one backslash each of its separators is written with.
    #[test]
    fn a_backslash_is_doubled_only_where_it_could_start_an_escape() {
        for text in [r"C:\Users\me\a.txt", r"a\b", r"a\ub", "a\\"] {
            assert_eq!(shown(text), text, "{text:?}");
        }
        assert_eq!(shown(r"a\u{202e}b"), r"a\\u{202e}b");
        assert_eq!(shown(r"a\u{}"), r"a\\u{}");
        // A literal backslash before a character this module escapes: the printed escape's own
        // backslash must not pair with the literal one.
        assert_eq!(shown("a\\\u{202e}"), r"a\\\u{202e}");
        assert_eq!(shown("a\\\u{1b}[2J"), r"a\\\u{1b}[2J");
    }

    /// Text that cannot change what a terminal shows is printed as it is, and text that can is not.
    #[test]
    fn every_character_that_can_change_a_terminal_is_escaped() {
        let text =
            "a\u{0}\u{1f}\u{7f}\u{9f}\u{61c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}b";
        let printed = shown(text);
        assert_eq!(
            printed,
            r"a\u{0}\u{1f}\u{7f}\u{9f}\u{61c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}b"
        );
        assert!(printed.is_ascii());
    }
}
