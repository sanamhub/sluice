//! Reads `tests/vectors/paths/` and checks each case, including the `rule` field. The conformance
//! runner in `xtask` checks `valid` only, so this is what keeps the rule numbers in the vectors
//! honest for an implementation that reports them.

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::check;

#[test]
fn every_case_in_the_path_vectors_gets_its_valid_flag_and_its_lowest_rule() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/paths");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("tests/vectors/paths exists")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty());

    let mut cases = 0;
    for file in files {
        let vectors: Value =
            serde_json::from_str(&fs::read_to_string(&file).expect("read")).expect("JSON");
        for case in vectors["cases"].as_array().expect("cases") {
            let path = case["path"].as_str().expect("path");
            let shown: String = path.chars().take(40).collect();
            let found = check(path).err().map(|v| u64::from(v.rule));
            let valid = case["valid"].as_bool().expect("valid");
            assert_eq!(found.is_none(), valid, "{}: {shown:?}", file.display());
            assert_eq!(
                found,
                case.get("rule").map(|r| r.as_u64().expect("rule")),
                "{}: {shown:?}",
                file.display()
            );
            cases += 1;
        }
    }
    assert!(cases >= 60, "{cases} cases");
}
