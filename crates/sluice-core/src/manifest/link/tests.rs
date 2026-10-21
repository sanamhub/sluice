use super::LinkTarget;
use crate::Error;
use crate::manifest::path::ManifestPath;

fn rule(text: &str) -> Option<u8> {
    LinkTarget::parse_rule(text).err()
}

#[test]
fn rule_1_empty_and_oversize() {
    let below = format!("{}a", "a/".repeat(2047));
    assert_eq!(below.len(), 4095);
    assert_eq!(rule("a"), None);
    assert_eq!(rule(&below), None);
    assert_eq!(rule(&format!("{below}b")), None);
    assert_eq!(rule(&format!("{below}bc")), Some(1));
    assert_eq!(rule(""), Some(1));
    // The limit counts bytes: 1366 three-byte characters are 4098.
    assert_eq!(rule(&"日".repeat(1366)), Some(1));
}

#[test]
fn rule_2_an_absolute_target_is_never_accepted() {
    for text in ["/", "/etc/passwd", "a/", "a/b/", "//server/share"] {
        assert_eq!(rule(text), Some(2), "{text:?}");
    }
    assert_eq!(rule("a/b"), None);
}

#[test]
fn rule_3_a_component_is_never_empty_or_a_dot() {
    for text in ["a//b", "./a", "a/./b", "a/.", ".", "a/b/."] {
        assert_eq!(rule(text), Some(3), "{text:?}");
    }
    // `..` is allowed here, and `.a` and `a.` are names.
    assert_eq!(rule("../a"), None);
    assert_eq!(rule(".a/..b"), None);
}

#[test]
fn rule_4_a_component_breaks_a_path_rule() {
    let rejected = [
        "C:\\x",
        "\\\\server\\share",
        "a\\b",
        "c:/x",
        "a/con",
        "a/NUL.txt",
        "a/b.",
        "a/b ",
        "a/<b>",
        "a/\u{202e}b",
        "a/e\u{301}",
        &"a".repeat(256),
    ];
    for text in rejected {
        assert_eq!(rule(text), Some(4), "{text:?}");
    }
    assert_eq!(rule(&"a".repeat(255)), None);
    assert_eq!(rule("a/CONSOLE/..a"), None);
}

#[test]
fn a_target_with_two_faults_names_the_lowest_rule() {
    assert_eq!(rule("/a//b"), Some(2));
    assert_eq!(rule("a//con"), Some(3));
}

#[test]
fn rule_5_does_not_step_above_the_output_folder() {
    let target = |text: &str| LinkTarget::parse(text).expect("passes rules 1 to 4");
    let link = |path: &str| ManifestPath::parse(path).expect("a path");
    // From the top, there is no folder above the link.
    assert!(!target("../x").stays_inside(&link("link")));
    assert!(target("x/../y").stays_inside(&link("link")));
    assert!(!target("x/../../y").stays_inside(&link("link")));
    // One folder down, one step up is the output folder.
    assert!(target("../b").stays_inside(&link("a/link")));
    assert!(!target("../../b").stays_inside(&link("a/link")));
    assert!(target("..").stays_inside(&link("a/link")));
    assert!(!target("../..").stays_inside(&link("a/link")));
    // Components are taken one at a time, so going down first gives room to come back.
    assert!(target("c/d/../../b").stays_inside(&link("a/link")));
}

#[test]
fn the_error_names_the_rule_and_not_the_text() {
    let err = LinkTarget::parse("/very-secret-name").unwrap_err();
    assert!(matches!(err, Error::Policy(_)));
    let message = err.to_string();
    assert!(message.contains("link target rule 2"), "{message}");
    assert!(!message.contains("secret"), "{message}");
}
