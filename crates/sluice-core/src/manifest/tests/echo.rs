//! A manifest comes from a stranger, and its errors reach logs. No error names a path or a link
//! target. For each rule that looks at one, a distinctive name is put into the manifest and the
//! error is searched for any 6 characters of it, in either case, the way the ticket tests do.

use super::common::*;
use crate::Error;
use crate::manifest::{Limits, Manifest};

/// The characters of `secret` the message shares a run of `window` with, if any.
fn echoed(message: &str, secret: &str, window: usize) -> Option<String> {
    let message = message.to_lowercase();
    let chars: Vec<char> = secret.to_lowercase().chars().collect();
    chars
        .windows(window)
        .map(|run| run.iter().collect::<String>())
        .find(|run| message.contains(run.as_str()))
}

fn assert_silent(err: &Error, secrets: &[&str]) {
    let message = err.to_string();
    for secret in secrets {
        assert_eq!(echoed(&message, secret, 6), None, "{message}");
    }
}

#[test]
fn the_finder_finds_a_run_and_ignores_a_short_one() {
    assert!(echoed("rule 3: quartz-lamp", "xx quartz-lamp xx", 6).is_some());
    assert_eq!(echoed("rule 3: quart", "quartz-lamp", 6), None);
    assert_eq!(echoed("QUARTZ", "quartz", 6), Some("quartz".into()));
}

const SECRET: &str = "vexwqzjk-plum";

#[test]
fn a_path_that_breaks_a_path_rule_is_not_in_the_error() {
    let bad_paths = [
        format!("{SECRET}//x"),
        format!("{SECRET}/../x"),
        format!("{SECRET}\\x"),
        format!("{SECRET}:x"),
        format!("{SECRET} "),
        format!("/{SECRET}"),
        format!("{SECRET}/con"),
        format!("{SECRET}\u{301}"),
        format!("{SECRET}{}", "x".repeat(300)),
    ];
    for path in &bad_paths {
        let err = decode(&manifest(&[file(path)])).unwrap_err();
        assert_eq!(rule_of(&err).0, 9, "{path:?}");
        assert_silent(&err, &[path]);
    }
}

#[test]
fn a_link_target_that_breaks_a_rule_is_not_in_the_error() {
    let bad = [
        format!("/{SECRET}"),
        format!("{SECRET}//x"),
        format!("{SECRET}\\x"),
        format!("{SECRET}/con"),
        format!("../../{SECRET}"),
    ];
    for target in &bad {
        let err = decode(&manifest(&[dir("d"), symlink("d/l", target)])).unwrap_err();
        assert_eq!(rule_of(&err).0, 11, "{target:?}");
        assert_silent(&err, &[target]);
    }
}

#[test]
fn a_path_in_any_other_fault_is_not_in_the_error() {
    let a = format!("{SECRET}-a");
    let b = format!("{SECRET}-A");
    let sorted_wrong = manifest(&[file(&b), file(&a)]);
    let repeated = manifest(&[file(&a), file(&a)]);
    let collide = manifest(&[file(&b), file(&a)]);
    let orphan = manifest(&[file(&format!("{SECRET}/{SECRET}"))]);
    let mut no_target = symlink(&a, "x");
    no_target.link = no_link();
    let mut not_utf8 = file(&a);
    not_utf8.path = [varint(14), SECRET.as_bytes().to_vec(), vec![0xff]].concat();
    let mut wrong_kind = file(&a);
    wrong_kind.loc = no_loc();
    let mut big_mode = file(&a);
    big_mode.mode = varint(0o200000);
    let packed_out = Raw {
        loc: packed(3, 0, 5),
        ..file(&a)
    };
    for bytes in [
        sorted_wrong,
        repeated,
        collide,
        orphan,
        manifest(&[no_target]),
        manifest(&[not_utf8]),
        manifest(&[wrong_kind]),
        manifest(&[big_mode]),
        manifest(&[packed_out]),
    ] {
        let err = decode(&bytes).unwrap_err();
        assert_silent(&err, &[SECRET]);
    }
}

#[test]
fn new_does_not_name_a_path_either() {
    use crate::manifest::{Entry, Kind, Loc};
    let entry = |text: &str| Entry {
        path: path(text),
        kind: Kind::File,
        size: 1,
        mtime_ms: 0,
        mode: 0o644,
        loc: Loc::Blob { root: [0; 32] },
        link_target: None,
    };
    let a = format!("{SECRET}-a");
    let err = Manifest::new(vec![entry(&a), entry(&a)], vec![]).unwrap_err();
    assert_silent(&err, &[SECRET]);
    let err = Manifest::new(vec![entry(&format!("{SECRET}/x"))], vec![]).unwrap_err();
    assert_silent(&err, &[SECRET]);
}

#[test]
fn the_limits_and_the_version_have_no_text_to_echo() {
    let err = Manifest::decode(&[2, 0, 0], Limits::default()).unwrap_err();
    assert_eq!(rule_of(&err), (3, None));
}
