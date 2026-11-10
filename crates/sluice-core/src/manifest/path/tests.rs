use super::{ManifestPath, check};
use crate::Error;

/// The number of the lowest rule `path` breaks, `None` if it passes.
fn rule(path: &str) -> Option<u8> {
    check(path).err().map(|v| v.rule)
}

fn accepts(paths: &[&str]) {
    for path in paths {
        assert_eq!(rule(path), None, "{path:?} must be accepted");
        assert!(ManifestPath::parse(path).is_ok(), "{path:?}");
    }
}

fn rejects(rule_number: u8, paths: &[&str]) {
    for path in paths {
        assert_eq!(
            rule(path),
            Some(rule_number),
            "{path:?} must break rule {rule_number}"
        );
        assert!(
            matches!(ManifestPath::parse(path), Err(Error::Policy(_))),
            "{path:?}"
        );
    }
}

#[test]
fn rule_1_empty_and_oversize() {
    // 2047 one-byte components with their slashes, then one more byte, is 4095 bytes.
    let below = format!("{}a", "a/".repeat(2047));
    assert_eq!(below.len(), 4095);
    accepts(&["a", &below, &format!("{below}b")]);
    // 1366 three-byte characters are 4098 bytes: the limit counts bytes, not characters.
    rejects(
        1,
        &[
            "",
            &format!("{below}bc"),
            &"a/".repeat(2049),
            &"日".repeat(1366),
        ],
    );
}

#[test]
fn rule_2_not_nfc() {
    // "e" with a combining acute, and "é" as one code point.
    rejects(
        2,
        &["e\u{301}", "dir/caf\u{65}\u{301}.txt", "\u{1100}\u{1161}"],
    );
    accepts(&["\u{e9}", "dir/caf\u{e9}.txt", "\u{ac00}", "日本語/ファイル"]);
    // A decomposed name wins over rule 3, which is the higher number.
    assert_eq!(rule("/e\u{301}"), Some(2));
}

#[test]
fn rule_3_slash_at_either_end() {
    rejects(3, &["/", "/a", "a/", "/a/b", "//a", "a//"]);
    accepts(&["a", "a/b"]);
}

#[test]
fn rule_4_empty_dot_and_dot_dot() {
    rejects(
        4,
        &[
            "a//b", ".", "..", "a/./b", "a/../b", "../a", "a/..", "a///b",
        ],
    );
    accepts(&["..a", "a/.hidden", "a...b", "...a", "a/..b"]);
}

#[test]
fn rule_5_component_in_bytes() {
    let ascii = "a".repeat(255);
    accepts(&[
        &ascii,
        &format!("d/{ascii}"),
        &format!("{}a", "\u{e9}".repeat(127)),
    ]);
    rejects(5, &[&"a".repeat(256), &format!("d/{}", "a".repeat(256))]);
    // 85 three-byte characters are 255 bytes, 86 are 258. 63 four-byte characters and "abc" are 255.
    accepts(&[&"日".repeat(85), &format!("{}abc", "😀".repeat(63))]);
    rejects(
        5,
        &[
            &"日".repeat(86),
            &"\u{e9}".repeat(128),
            &format!("{}abcd", "😀".repeat(63)),
        ],
    );
}

#[test]
fn rule_6_forbidden_characters() {
    for c in [
        '\\', ':', '<', '>', '"', '|', '?', '*', '\0', '\u{1}', '\n', '\r', '\t', '\u{1b}',
        '\u{1f}', '\u{7f}',
    ] {
        let path = format!("a{c}b");
        assert_eq!(rule(&path), Some(6), "{path:?}");
    }
    rejects(6, &["a\\b", "C:/a", "C:", "file.txt:stream", "dir/a?b.txt"]);
    // The C1 controls, all 32 of them, and the edges around them.
    for c in '\u{80}'..='\u{9f}' {
        let path = format!("a{c}b");
        assert_eq!(rule(&path), Some(6), "{path:?}");
    }
    accepts(&["a\u{a0}b", "a\u{ff}b", "a\u{7e}b"]);
}

#[test]
fn rule_6_bidirectional_controls() {
    let bidi = [
        '\u{61c}', '\u{200e}', '\u{200f}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}',
        '\u{202e}', '\u{2066}', '\u{2067}', '\u{2068}', '\u{2069}',
    ];
    for c in bidi {
        let path = format!("invoice{c}txt.exe");
        assert_eq!(rule(&path), Some(6), "{path:?}");
    }
    // The neighbours: the zero-width characters that emoji sequences and Persian need, the
    // marks next to the bidirectional range, and ordinary text in a right-to-left script.
    accepts(&[
        "a\u{200b}b",
        "a\u{200c}b",
        "a\u{200d}b",
        "a\u{feff}b",
        "a\u{200d}",
        "a\u{2060}b",
        "a\u{2065}b",
        "a\u{206a}b",
        "a\u{2029}b",
        "a\u{202f}b",
        "\u{645}\u{644}\u{641}.txt",
    ]);
}

#[test]
fn rule_6_beats_rules_7_and_8() {
    // A C1 control in a name that is also a device name: rule 6 comes before rule 8.
    assert_eq!(rule("con\u{85}"), Some(6));
    assert_eq!(rule("a\u{202e}."), Some(6));
}

#[test]
fn the_lowest_rule_wins_when_a_path_breaks_several() {
    assert_eq!(rule("/con"), Some(3));
    assert_eq!(rule("con:"), Some(6));
    assert_eq!(rule("../con."), Some(4));
    assert_eq!(rule("a\\b."), Some(6));
    assert_eq!(rule("con."), Some(7));
    assert_eq!(rule(&format!("{}:", "a".repeat(300))), Some(5));
    assert_eq!(rule(&format!("{}/", "a".repeat(5000))), Some(1));
    // The lowest across components, not the first one met.
    assert_eq!(rule("con/a//b"), Some(4));
    assert_eq!(rule("a./b:"), Some(6));
}

#[test]
fn the_error_names_the_rule_and_the_component() {
    let message = |path: &str| ManifestPath::parse(path).unwrap_err().to_string();
    assert_eq!(
        message("a/../b"),
        r#"policy denied: manifest path rule 4: component ".." is not a name"#
    );
    assert_eq!(
        message("a//b"),
        r#"policy denied: manifest path rule 4: component "" is empty"#
    );
    assert_eq!(
        message("x/NUL.txt"),
        r#"policy denied: manifest path rule 8: component "NUL.txt" is a Windows reserved device name"#
    );
    assert_eq!(
        message("a\nb"),
        r#"policy denied: manifest path rule 6: component "a\nb" contains '\n'"#
    );
    assert_eq!(
        message(""),
        "policy denied: manifest path rule 1: the path is empty"
    );
    assert_eq!(
        message("/a"),
        "policy denied: manifest path rule 3: the path begins with `/`"
    );
}

#[test]
fn the_error_never_carries_a_long_path() {
    let component = "x".repeat(255);
    let long_path = format!("{}/..", vec![component.as_str(); 15].join("/"));
    assert!(long_path.len() > 255);
    for path in [
        long_path,
        format!("{}:", "x".repeat(255)),
        format!("{}\n", "x".repeat(255)),
        "a".repeat(5000),
        format!("{}/", "\u{1b}".repeat(300)),
    ] {
        let message = ManifestPath::parse(&path).unwrap_err().to_string();
        assert!(
            message.len() < 400,
            "{} bytes for a path of {}",
            message.len(),
            path.len()
        );
        assert!(
            !message.contains(&component),
            "the message quotes a whole component"
        );
    }
}

#[test]
fn an_accepted_path_is_kept_as_written() {
    let path = ManifestPath::parse("a/b/\u{65e5}\u{672c}.txt").unwrap();
    assert_eq!(path.as_str(), "a/b/\u{65e5}\u{672c}.txt");
    assert_eq!(
        path.components().collect::<Vec<_>>(),
        ["a", "b", "\u{65e5}\u{672c}.txt"]
    );
    assert_eq!(path.clone(), path);
}

mod properties;
mod reserved;
