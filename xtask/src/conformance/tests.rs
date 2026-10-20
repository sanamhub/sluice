use std::fs;
use std::path::{Path, PathBuf};

use super::{check, vectors_dir};

/// A directory removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sluice-xtask-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    fn write(&self, relative: &str, text: &str) {
        let file = self.0.join(relative);
        fs::create_dir_all(file.parent().expect("a parent")).expect("create dir");
        fs::write(file, text).expect("write file");
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_checked_in_vectors_pass() {
    let report = check(&vectors_dir()).expect("read the vectors");
    assert_eq!(report.failed, 0, "{}", report.lines.join("\n"));
    assert!(
        report.passed_in("paths") >= 60 && report.passed_in("tickets") >= 40,
        "{}",
        report.lines.join("\n")
    );
}

#[test]
fn a_case_the_implementation_disagrees_with_is_printed_with_its_description() {
    let dir = TempDir::new("disagree");
    dir.write(
        "paths/x.json",
        r#"{ "description": "two cases, one wrong", "cases": [
            { "path": "a/b", "valid": true },
            { "path": "a/b", "valid": false, "rule": 4 } ] }"#,
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.failed, 1);
    assert_eq!(report.passed_in("paths"), 1);
    let text = report.lines.join("\n");
    assert!(text.contains("paths/x.json  1 of 2 passed"), "{text}");
    assert!(
        text.contains(
            r#"FAIL "a/b" (3 bytes): expected rejected, got accepted. two cases, one wrong"#
        ),
        "{text}"
    );
}

#[test]
fn a_long_path_is_cut_in_the_failure_line() {
    let dir = TempDir::new("long");
    let path = "a".repeat(255);
    dir.write(
        "paths/x.json",
        &format!(r#"{{ "description": "d", "cases": [ {{ "path": "{path}", "valid": false, "rule": 5 }} ] }}"#),
    );
    let text = check(dir.path()).expect("run").lines.join("\n");
    assert!(text.contains("... (255 bytes)"), "{text}");
    assert!(!text.contains(&path), "{text}");
}

#[test]
fn a_directory_without_a_runner_is_skipped_and_not_counted() {
    let dir = TempDir::new("skipped");
    dir.write("manifests/x.json", r#"{ "description": "d", "cases": [] }"#);
    dir.write(
        "paths/x.json",
        r#"{ "description": "d", "cases": [ { "path": "a", "valid": true } ] }"#,
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.skipped, ["manifests"]);
    assert_eq!(report.failed, 0);
    assert_eq!(report.passed_in("paths"), 1);
    assert_eq!(report.passed_in("manifests"), 0);
    assert!(
        report
            .lines
            .iter()
            .any(|l| l.starts_with("manifests/") && l.contains("skipped"))
    );
}

#[test]
fn a_file_that_cannot_be_read_is_a_failure() {
    for (name, body) in [
        ("not-json", "{"),
        ("no-cases", r#"{ "description": "d", "cases": [] }"#),
        (
            "no-description",
            r#"{ "cases": [ { "path": "a", "valid": true } ] }"#,
        ),
        (
            "no-valid",
            r#"{ "description": "d", "cases": [ { "path": "a" } ] }"#,
        ),
    ] {
        let dir = TempDir::new(name);
        dir.write("paths/x.json", body);
        let report = check(dir.path()).expect("run");
        assert_eq!(report.failed, 1, "{name}: {:?}", report.lines);
    }
}

#[test]
fn an_empty_paths_directory_is_a_failure() {
    let dir = TempDir::new("empty");
    fs::create_dir_all(dir.path().join("paths")).expect("create dir");
    assert_eq!(check(dir.path()).expect("run").failed, 1);
}

const TICKET: &str = "sluice1040";

/// A valid case for a Live ticket whose fields are all zero, with one field changed by the test.
fn ticket_case(text: &str, canonical: &str, relay: &str) -> String {
    let zeros = "00".repeat(32);
    format!(
        r#"{{ "description": "tickets", "cases": [ {{ "ticket": "{text}", "valid": true, "canonical": "{canonical}",
        "fields": {{ "version": 1, "kind": "live", "endpoint_id": "{zeros}", "addrs": [], "relay_url": {relay},
        "expiry_ms": null, "stream_id": "{}", "sender_key": "{zeros}" }} }} ] }}"#,
        "00".repeat(16)
    )
}

fn live_text() -> String {
    use sluice_core::ticket::{Body, Ticket};
    let body = Body::Live {
        stream_id: [0; 16],
        sender_key: [0; 32],
    };
    Ticket::new([0; 32], vec![], None, None, body)
        .expect("a ticket")
        .encode()
}

#[test]
fn a_ticket_case_that_matches_passes_in_any_case_and_one_that_does_not_is_printed() {
    let lower = live_text();
    assert!(lower.starts_with(TICKET));
    let dir = TempDir::new("tickets-ok");
    dir.write(
        "tickets/x.json",
        &ticket_case(&lower.to_uppercase(), &lower, "null"),
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(
        (report.failed, report.passed_in("tickets")),
        (0, 1),
        "{:?}",
        report.lines
    );

    for (name, case, wanted) in [
        (
            "canonical",
            ticket_case(&lower, &lower.to_uppercase(), "null"),
            "other text than `canonical`",
        ),
        (
            "fields",
            ticket_case(&lower, &lower, r#""r""#),
            "decoded to other fields",
        ),
        (
            "rejected",
            ticket_case("sluice1", "sluice1", "null"),
            "expected accepted",
        ),
    ] {
        let dir = TempDir::new(name);
        dir.write("tickets/x.json", &case);
        let report = check(dir.path()).expect("run");
        assert_eq!(report.failed, 1, "{name}");
        let text = report.lines.join("\n");
        assert!(
            text.contains(wanted) && text.contains("tickets"),
            "{name}: {text}"
        );
    }
}

#[test]
fn an_invalid_ticket_case_that_decodes_and_a_valid_one_without_fields_are_failures() {
    let dir = TempDir::new("tickets-invalid");
    let lower = live_text();
    dir.write(
        "tickets/x.json",
        &format!(
            r#"{{ "description": "d", "cases": [
                {{ "ticket": "{lower}", "valid": false }},
                {{ "ticket": "{lower}", "valid": true }} ] }}"#
        ),
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.failed, 2, "{:?}", report.lines);
    let text = report.lines.join("\n");
    assert!(text.contains("expected rejected, got accepted"), "{text}");
    assert!(text.contains("needs `canonical` and `fields`"), "{text}");
}
