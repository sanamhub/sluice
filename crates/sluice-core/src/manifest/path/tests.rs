use proptest::prelude::*;

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
    // Neighbours of the control range and the C1 controls are plain characters to the rules.
    accepts(&[
        "a b", "a!b", "a#b", "a~b", "a\u{80}b", "a\u{a0}b", "a\u{ff}b", "a;b", "a'b",
    ]);
}

#[test]
fn rule_7_trailing_dot_or_space() {
    rejects(
        7,
        &[
            "a.", "a ", "dir./a", "dir /a", "a/b.", "a/b ", "...", "a. ", " ",
        ],
    );
    accepts(&["a.b", " a", "a b", "a/ b", "a/.b"]);
}

#[test]
fn rule_8_reserved_device_names() {
    rejects(
        8,
        &[
            "con",
            "CON",
            "Con",
            "prn",
            "AUX",
            "nul",
            "Nul.txt",
            "CONIN$",
            "conout$",
            "ConIn$.log",
            "com0",
            "COM1",
            "com9",
            "LPT0",
            "lpt9",
            "COM\u{b9}",
            "LPT\u{b2}.log",
            "com\u{b3}",
            "LPT\u{b9}.log",
            "a/con",
            "con/a",
            "dir/NUL.tar.gz",
            "CON .txt",
            "con  .txt",
            "CON.tar.gz",
            // U+0131 folds to `I`.
            "CONIN$".replace('I', "\u{131}").as_str(),
        ],
    );
    accepts(&[
        "CONSOLE",
        "con_1",
        "CON_1",
        "COM",
        "COM10",
        "COM1a",
        "LPT",
        "LPT10",
        "COMx",
        "AUXILIARY",
        "NULL",
        "PRN1",
        "CONIN",
        "CONOUT",
        "CONIN$$",
        "xcon",
        "con-",
        "a.con",
        "a.nul",
        ".con",
        "CON\u{e9}",
        "COM\u{b9}\u{b9}",
    ]);
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

/// One component that is often fine and sometimes breaks a rule, so the accepted and the
/// rejected side of every rule both get exercised.
fn component() -> impl Strategy<Value = String> {
    prop_oneof![
        8 => "[a-z0-9\u{e9}\u{65e5}\u{672c}_-]{1,12}",
        2 => "[a-zA-Z0-9._ -]{1,12}",
        1 => "[.:\\\\ a]{0,3}",
        1 => "(con|nul|aux|prn|com[0-9\u{b9}]|lpt[0-9\u{b2}])(\\.[a-z]{1,3})?",
    ]
}

proptest! {
    #[test]
    fn an_accepted_path_is_made_of_accepted_components(parts in prop::collection::vec(component(), 1..6)) {
        let text = parts.join("/");
        if let Ok(path) = ManifestPath::parse(&text) {
            for part in path.components() {
                prop_assert!(ManifestPath::parse(part).is_ok(), "component {part:?} of {text:?}");
            }
            prop_assert_eq!(path.components().collect::<Vec<_>>().join("/"), text);
        }
    }

    #[test]
    fn a_path_of_the_awkward_characters_is_accepted_or_rejected(text in "[a./\\\\: ]{0,24}") {
        if let Ok(path) = ManifestPath::parse(&text) {
            for part in path.components() {
                prop_assert!(!part.is_empty() && part != "." && part != "..", "{text:?}");
            }
            prop_assert!(!text.contains(['\\', ':']), "{text:?}");
        }
    }

    #[test]
    fn any_text_gets_an_answer_and_a_short_error(text in any::<String>()) {
        if let Err(Error::Policy(message)) = ManifestPath::parse(&text) {
            prop_assert!(message.len() < 400, "{} bytes", message.len());
        }
    }
}
