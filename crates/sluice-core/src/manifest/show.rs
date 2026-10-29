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
/// U+202E. Every backslash in a run is therefore doubled when that run ends where an escape could
/// follow — the literal `u{`, or a character this function writes as `\u{…}` — and a run that ends
/// anywhere else is kept as it is. Doubling the whole run, not only its last backslash, is what
/// keeps two different names from printing alike; doubling every backslash everywhere would make
/// every Windows path in a message read `C:\\Users\\…`, and a reader would have no way to tell
/// which of the two backslashes the sender's text held.
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
    let mut at = 0;
    while at < text.len() {
        let c = text[at..]
            .chars()
            .next()
            .expect("an index into the string is on a character");
        if c == '\\' {
            let mut end = at;
            while text[end..].starts_with('\\') {
                end += 1;
            }
            if ends_where_an_escape_could_follow(text, end) {
                for _ in at..end {
                    out.push_str(r"\\");
                }
            } else {
                out.push_str(&text[at..end]);
            }
            at = end;
            continue;
        }
        if is_display_control(c) || is_line_separator(c) {
            out.push_str(&format!("\\u{{{:x}}}", u32::from(c)));
        } else {
            out.push(c);
        }
        at += c.len_utf8();
    }
    out
}

/// Whether an escape this module writes could start at byte `end`, which is just past a run of
/// backslashes: the literal `u{`, or a character the next step would write as `\u{…}`.
fn ends_where_an_escape_could_follow(text: &str, end: usize) -> bool {
    let rest = &text[end..];
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

    /// A backslash is doubled only in a run that ends where an escape could follow, so a Windows
    /// path keeps the one backslash each of its separators is written with.
    #[test]
    fn a_backslash_is_doubled_only_where_it_could_start_an_escape() {
        for text in [
            r"C:\Users\me\a.txt",
            r"a\b",
            r"a\ub",
            "a\\",
            r"a\b\u",
            r"a\",
        ] {
            assert_eq!(shown(text), text, "{text:?}");
        }
        assert_eq!(shown(r"a\u{202e}b"), r"a\\u{202e}b");
        assert_eq!(shown(r"a\u{}"), r"a\\u{}");
        // A literal backslash before a character this module escapes: the escape's own backslash
        // must not pair with the literal one, so the whole run is doubled.
        assert_eq!(shown("a\\\u{202e}"), r"a\\\u{202e}");
        assert_eq!(shown("a\\\u{1b}[2J"), r"a\\\u{1b}[2J");
    }

    /// The run is the whole run: with only its last backslash doubled, two different names printed
    /// alike, one of `k` backslashes and the literal text `u{202e}`, and one of `k - 1` backslashes
    /// and U+202E.
    #[test]
    fn a_run_of_two_or_three_backslashes_does_not_collide_with_an_escape() {
        for k in [2usize, 3] {
            let literal = format!("{}u{{202e}}", "\\".repeat(k));
            let escaped = format!("{}\u{202e}", "\\".repeat(k - 1));
            assert_ne!(literal, escaped);
            assert_ne!(
                shown(&literal),
                shown(&escaped),
                "two names printed alike for {k} backslashes"
            );
        }
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

    /// Two different names never print alike. The alphabet holds the characters the escaping rules
    /// are about: backslashes, the letters and braces of an escape, hex digits, the controls that
    /// are written as one, and an ordinary letter and control.
    #[test]
    fn two_names_never_print_alike() {
        use proptest::prelude::*;

        const ALPHABET: &[char] = &[
            '\\', 'u', '{', '}', '0', '1', 'a', 'f', '\u{202e}', '\u{2028}', '\u{1b}', 'x',
        ];
        let text = || {
            prop::collection::vec(prop::sample::select(ALPHABET.to_vec()), 0..16)
                .prop_map(|chars| chars.into_iter().collect::<String>())
        };
        proptest!(|(a in text(), b in text())| {
            if shown(&a) == shown(&b) {
                prop_assert_eq!(a, b);
            }
        });
    }
}
