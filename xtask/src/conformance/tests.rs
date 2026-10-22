use std::fs;
use std::path::{Path, PathBuf};

use super::{check, vectors_dir};

/// A directory removed on drop.
pub(super) struct TempDir(PathBuf);

impl TempDir {
    pub(super) fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("sluice-xtask-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        Self(dir)
    }

    pub(super) fn write(&self, relative: &str, text: &str) {
        let file = self.0.join(relative);
        fs::create_dir_all(file.parent().expect("a parent")).expect("create dir");
        fs::write(file, text).expect("write file");
    }

    pub(super) fn path(&self) -> &Path {
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
        report.passed_in("paths") >= 60
            && report.passed_in("tickets") >= 40
            && report.passed_in("manifests") >= 60
            && report.passed_in("packs") >= 20
            && report.passed_in("live") >= 60,
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
        &format!(
            r#"{{ "description": "d", "cases": [
            {{ "path": "{path}", "valid": false, "rule": 5 }} ] }}"#
        ),
    );
    let text = check(dir.path()).expect("run").lines.join("\n");
    assert!(text.contains("... (255 bytes)"), "{text}");
    assert!(!text.contains(&path), "{text}");
}

#[test]
fn a_directory_without_a_runner_is_skipped_and_not_counted() {
    let dir = TempDir::new("skipped");
    dir.write("delta/x.json", r#"{ "description": "d", "cases": [] }"#);
    dir.write(
        "paths/x.json",
        r#"{ "description": "d", "cases": [ { "path": "a", "valid": true } ] }"#,
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.skipped, ["delta"]);
    assert_eq!(report.failed, 0);
    assert_eq!(report.passed_in("paths"), 1);
    assert_eq!(report.passed_in("delta"), 0);
    assert!(
        report
            .lines
            .iter()
            .any(|l| l.starts_with("delta/") && l.contains("skipped"))
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

/// A valid case for a Live ticket whose keys are 0x11 bytes and whose other fields are zero, with
/// one field changed by the test. Rule 8 refuses a key of zeros, which is of small order.
fn ticket_case(text: &str, canonical: &str, relay: &str) -> String {
    let key = "11".repeat(32);
    format!(
        r#"{{ "description": "tickets", "cases": [ {{ "ticket": "{text}", "valid": true,
        "canonical": "{canonical}",
        "fields": {{ "version": 1, "kind": "live", "endpoint_id": "{key}",
        "addrs": [], "relay_url": {relay}, "expiry_ms": null, "stream_id": "{}",
        "sender_key": "{key}" }} }} ] }}"#,
        "00".repeat(16)
    )
}

fn live_text() -> String {
    use sluice_core::ticket::{Body, Ticket};
    let body = Body::Live {
        stream_id: [0; 16],
        sender_key: [0x11; 32],
    };
    Ticket::new([0x11; 32], vec![], None, None, body)
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
            ticket_case(&lower, &lower, r#""https://relay.example/""#),
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

/// The empty manifest, as a valid case.
const EMPTY_CASE: &str = r#"{ "name": "empty", "bytes": "010000", "valid": true,
    "root": "396eab8ebe8b2247fed631cee54835e0c8678ab1cc0d5b8a73f9f86e813187d5",
    "fields": { "version": 1, "entries": [], "packs": [] } }"#;

fn manifest_report(name: &str, cases: &[&str]) -> super::Report {
    let dir = TempDir::new(name);
    dir.write(
        "manifests/x.json",
        &format!(
            r#"{{ "description": "d", "cases": [ {} ] }}"#,
            cases.join(", ")
        ),
    );
    check(dir.path()).expect("run")
}

#[test]
fn a_valid_manifest_case_must_decode_encode_and_hash_as_written() {
    let report = manifest_report("manifest-valid", &[EMPTY_CASE]);
    assert_eq!(
        report.failed,
        0,
        "{}",
        report.lines.join(
            "
"
        )
    );
    assert_eq!(report.passed_in("manifests"), 1);

    let wrong_root = EMPTY_CASE.replace("396eab", "396eac");
    let wrong_fields = EMPTY_CASE.replace(
        r#""entries": []"#,
        r#""entries": [ { "path": "a", "kind": "dir", "size": "0", "mtime_ms": "0", "mode": 0,
            "loc": { "type": "none" }, "link_target": null } ]"#,
    );
    let wrong_bytes = EMPTY_CASE.replace(r#""010000""#, r#""01000000""#);
    for (case, expect) in [
        (wrong_root, "the root is not the one"),
        (wrong_fields, "decoded to other fields"),
        (wrong_bytes, "expected accepted"),
    ] {
        let text = manifest_report("manifest-wrong", &[&case]).lines.join(
            "
",
        );
        assert!(text.contains(expect), "{text}");
        assert!(text.contains(r#"FAIL "empty""#), "{text}");
    }
}

#[test]
fn an_invalid_manifest_case_must_be_rejected_with_the_limits_it_names() {
    let refused = r#"{ "name": "version 2", "bytes": "020000", "valid": false }"#;
    let accepted = r#"{ "name": "wrongly invalid", "bytes": "010000", "valid": false }"#;
    // The empty manifest is 3 bytes. A limit of 2 makes it too long, and a limit of 3 does not.
    let over = r#"{ "name": "over", "bytes": "010000", "valid": false,
        "limits": { "max_bytes": "2", "max_entries": "10" } }"#;
    let at = r#"{ "name": "at", "bytes": "010000", "valid": false,
        "limits": { "max_bytes": "3", "max_entries": "10" } }"#;
    let report = manifest_report("manifest-invalid", &[refused, accepted, over, at]);
    assert_eq!(
        report.failed,
        2,
        "{}",
        report.lines.join(
            "
"
        )
    );
    let text = report.lines.join(
        "
",
    );
    assert!(
        text.contains(r#"FAIL "wrongly invalid": expected rejected"#),
        "{text}"
    );
    assert!(text.contains(r#"FAIL "at": expected rejected"#), "{text}");
}

/// One small file in one pack, with its content and every root.
const PACK_CASE: &str = r#"{ "name": "one small file", "entries": [ { "path": "hello.txt",
    "kind": "file", "size": "5", "content": "68656c6c6f", "mtime_ms": "1800000000000", "mode": 420,
    "loc": { "type": "packed", "pack_index": 0, "offset": "0", "len": "5" } } ],
    "packs": [ { "size": "5",
        "root": "ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f" } ],
    "manifest_root": "1477ec03fbf6492d49a86cb4803e30befec193afbe39f35e1dd0d01ebfb3e3b7" }"#;

fn pack_report(name: &str, case: &str) -> super::Report {
    let dir = TempDir::new(name);
    dir.write(
        "packs/x.json",
        &format!(r#"{{ "description": "d", "cases": [ {case} ] }}"#),
    );
    check(dir.path()).expect("run")
}

#[test]
fn a_pack_case_must_lay_out_hash_and_unpack_as_written() {
    let report = pack_report("pack-ok", PACK_CASE);
    assert_eq!(report.failed, 0, "{:?}", report.lines);
    assert_eq!(report.passed_in("packs"), 1);
    for (name, from, to, expect) in [
        (
            "offset",
            r#""offset": "0""#,
            r#""offset": "1""#,
            "is placed",
        ),
        (
            "pack-size",
            r#"{ "size": "5","#,
            r#"{ "size": "6","#,
            "pack sizes",
        ),
        ("pack-root", "ea8f163d", "ea8f163e", "pack root"),
        ("manifest-root", "1477ec03", "1477ec04", "manifest root"),
        ("content", "68656c6c6f", "68656c6c70", "pack root"),
    ] {
        assert!(PACK_CASE.contains(from), "{name}");
        let text = pack_report(name, &PACK_CASE.replacen(from, to, 1))
            .lines
            .join("\n");
        assert!(text.contains(expect), "{name}: {text}");
        assert!(text.contains(r#"FAIL "one small file""#), "{name}: {text}");
    }
}

#[test]
fn a_pack_case_without_content_checks_the_layout_alone() {
    let case = r#"{ "name": "two files", "entries": [
        { "path": "a", "kind": "file", "size": "3",
          "loc": { "type": "packed", "pack_index": 0, "offset": "0", "len": "3" } },
        { "path": "b", "kind": "file", "size": "1048576", "loc": { "type": "blob" } } ],
        "packs": [ { "size": "3" } ] }"#;
    let report = pack_report("pack-layout", case);
    assert_eq!(report.failed, 0, "{:?}", report.lines);
    let wrong = case.replace(r#""type": "blob""#, r#""type": "none""#);
    assert_eq!(pack_report("pack-layout-wrong", &wrong).failed, 1);
}
