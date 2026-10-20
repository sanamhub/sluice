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
        report.passed_in("paths") >= 60,
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
    dir.write("tickets/x.json", r#"{ "description": "d", "cases": [] }"#);
    dir.write(
        "paths/x.json",
        r#"{ "description": "d", "cases": [ { "path": "a", "valid": true } ] }"#,
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.skipped, ["tickets"]);
    assert_eq!(report.failed, 0);
    assert_eq!(report.passed_in("paths"), 1);
    assert_eq!(report.passed_in("tickets"), 0);
    assert!(
        report
            .lines
            .iter()
            .any(|l| l.starts_with("tickets/") && l.contains("skipped"))
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
