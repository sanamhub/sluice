//! Reads `tests/vectors/manifests/` and checks the `rule` and `sub_rule` of every rejected case.
//! The conformance runner in `xtask` checks only that a case is rejected, so this is what keeps
//! the rule numbers in the vectors honest, and what shows that every decoding rule has a case.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;

use super::common::rule_of;
use crate::Error;
use crate::manifest::{Limits, Manifest};

fn hex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16).expect("hex"))
        .collect()
}

fn limits(case: &Value) -> Limits {
    let Some(limits) = case.get("limits") else {
        return Limits::default();
    };
    let field = |name: &str| {
        limits[name]
            .as_str()
            .expect(name)
            .parse()
            .expect("a number")
    };
    Limits {
        max_bytes: field("max_bytes"),
        max_entries: field("max_entries"),
    }
}

/// Every vector file, in name order.
fn files() -> Vec<Value> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/manifests");
    let mut paths: Vec<_> = fs::read_dir(&dir)
        .expect("tests/vectors/manifests exists")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("JSON"))
        .collect()
}

/// A valid case decodes, encodes to its own bytes and has the root it names.
fn check_valid(name: &str, bytes: &[u8], case: &Value, decoded: Result<Manifest, Error>) {
    let manifest = decoded.unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(manifest.encode(), bytes, "{name}");
    let root = hex(case["root"].as_str().expect("root"));
    assert_eq!(manifest.root().to_vec(), root, "{name}");
}

/// A rejected case breaks the rule it names, and the path rule or link target rule inside it.
fn check_rejected(name: &str, case: &Value, decoded: Result<Manifest, Error>) -> (u8, Option<u8>) {
    let err = decoded.err().unwrap_or_else(|| panic!("{name}: accepted"));
    let rule = case["rule"].as_u64().expect("rule") as u8;
    let sub = case.get("sub_rule").map(|s| s.as_u64().expect("sub") as u8);
    assert_eq!(rule_of(&err), (rule, sub), "{name}");
    (rule, sub)
}

#[test]
fn every_rejected_case_breaks_the_rule_it_names_and_every_rule_has_a_case() {
    let (mut valid, mut rejected) = (0, 0);
    let mut rules = BTreeSet::new();
    for file in files() {
        for case in file["cases"].as_array().expect("cases") {
            let name = case["name"].as_str().expect("name");
            let bytes = hex(case["bytes"].as_str().expect("bytes"));
            let decoded = Manifest::decode(&bytes, limits(case));
            if case["valid"].as_bool().expect("valid") {
                check_valid(name, &bytes, case, decoded);
                valid += 1;
            } else {
                rules.insert(check_rejected(name, case, decoded));
                rejected += 1;
            }
        }
    }
    assert!(
        valid >= 20 && rejected >= 100,
        "{valid} valid, {rejected} rejected"
    );
    for rule in 1..=15u8 {
        assert!(
            rules.iter().any(|(r, _)| *r == rule),
            "no case for rule {rule}"
        );
    }
    for sub in 1..=8u8 {
        assert!(
            rules.contains(&(9, Some(sub))),
            "no case for path rule {sub}"
        );
    }
    for sub in 1..=5u8 {
        assert!(
            rules.contains(&(11, Some(sub))),
            "no case for link target rule {sub}"
        );
    }
}
