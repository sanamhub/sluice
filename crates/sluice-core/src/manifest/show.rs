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
/// These are the characters of [`is_display_control`], the two Unicode line separators, and the
/// backslash. A manifest path cannot hold any of the first kind; a live stream's name is a file name
/// on the sender's disk, which on Unix may hold a backslash. The backslash is doubled so that a name
/// holding the four characters `\u{202e}` cannot be read as the escape written for U+202E: the
/// printed line uses `\` as the introducer of an escape, so a literal one has to be written twice.
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
/// assert_eq!(shown("a\u{2028}b\u{2029}c"), r"a\u{2028}b\u{2029}c");
/// ```
pub fn shown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c == '\\' {
            out.push_str(r"\\");
        } else if is_display_control(c) || is_line_separator(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
    }
    out
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
}
