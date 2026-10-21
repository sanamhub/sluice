//! The live runner compares every step and the end, so a vector that is wrong in any of them
//! fails. The cases are copies of checked-in vectors with one thing changed.

use serde_json::Value;

use super::tests::TempDir;
use super::{Report, check, vectors_dir};

/// The checked-in case named `name` in `live/file`.
fn case(file: &str, name: &str) -> Value {
    let text = std::fs::read_to_string(vectors_dir().join("live").join(file)).expect("read");
    let vectors: Value = serde_json::from_str(&text).expect("json");
    vectors["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["name"] == name)
        .cloned()
        .expect("the case")
}

fn report(tag: &str, case: &Value) -> Report {
    let dir = TempDir::new(tag);
    let file = serde_json::json!({ "description": "d", "cases": [case] });
    dir.write("live/x.json", &file.to_string());
    check(dir.path()).expect("run")
}

const SHORT: &str = "three segments, the last one short";

/// A name for the failure line, and one change to a case.
type Change = (&'static str, fn(&mut Value));

#[test]
fn a_live_case_passes_as_written() {
    let report = report("live-ok", &case("clean.json", SHORT));
    assert_eq!(report.failed, 0, "{:?}", report.lines);
    assert_eq!(report.passed_in("live"), 1);
}

#[test]
fn a_wrong_step_or_end_fails_the_case() {
    let changes: [Change; 5] = [
        ("seq", |c| c["steps"][1]["seq"] = "1".into()),
        ("outcome", |c| c["steps"][2]["outcome"] = "repeat".into()),
        ("total", |c| c["final"]["total_len"] = "1".into()),
        ("bytes", |c| {
            let bytes = c["steps"][1]["bytes"]
                .as_str()
                .expect("hex")
                .replacen('0', "1", 1);
            c["steps"][1]["bytes"] = bytes.into();
        }),
        ("order", |c| {
            c["steps"].as_array_mut().expect("steps").remove(0);
        }),
    ];
    for (tag, change) in changes {
        let mut wrong = case("clean.json", SHORT);
        change(&mut wrong);
        let report = report(&format!("live-{tag}"), &wrong);
        assert_eq!(report.failed, 1, "{tag}: {:?}", report.lines);
        let text = report.lines.join("\n");
        assert!(text.contains(&format!("FAIL \"{SHORT}\"")), "{tag}: {text}");
    }
}

#[test]
fn a_refusal_is_compared_by_its_rule() {
    let mut wrong = case("open.json", "an open signed for another stream");
    assert_eq!(report("live-rule-ok", &wrong).failed, 0);
    wrong["steps"][0]["rule"] = 2.into();
    let text = report("live-rule", &wrong).lines.join("\n");
    assert!(
        text.contains("got refused rule 3, want refused rule 2"),
        "{text}"
    );
}
