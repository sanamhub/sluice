//! `sluice doctor` through the built binary: what it prints, and the exit code a script gets.

use std::fs;
use std::path::Path;

mod common;

use common::{sluice, stderr_text, stdout_text};

/// A home as `sluice id` leaves it.
fn ready_home(dir: &Path) -> std::path::PathBuf {
    let home = dir.join("home");
    sluice(&home).arg("id").assert().success();
    home
}

/// The output with what changes from run to run or machine to machine put in brackets: the home's
/// path, the version, the target and the system, pids, and the whole free space line, whose status
/// depends on the disk the test runs on.
fn redact(text: &str, home: &Path) -> String {
    let shown = home.display().to_string();
    let escaped = serde_json::to_string(&shown).expect("a string");
    let mut text = text
        .replace(&escaped[1..escaped.len() - 1], "<home>")
        .replace(&shown, "<home>")
        .replace(env!("CARGO_PKG_VERSION"), "<version>")
        .replace(env!("SLUICE_TARGET"), "<target>")
        .replace(std::env::consts::OS, "<os>");
    let mut from = 0;
    while let Some(found) = text[from..].find("(pid ") {
        let at = from + found;
        let end = at + text[at..].find(')').expect("a closing bracket");
        text.replace_range(at..=end, "(pid <pid>)");
        from = at + "(pid <pid>)".len();
    }
    redact_space(&text)
}

fn redact_space(text: &str) -> String {
    if text.starts_with('{') {
        let start = text.find("{\"name\":\"space\"").expect("a space check");
        let end = start + text[start..].find('}').expect("its end") + 1;
        if text[start..end].contains("\"skipped\"") {
            return text.to_string();
        }
        let placeholder =
            "{\"name\":\"space\",\"status\":\"<status>\",\"detail\":\"<free>\",\"fix\":<fix>}";
        return format!("{}{placeholder}{}", &text[..start], &text[end..]);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut skip_fix = false;
    for line in text.lines() {
        if skip_fix && line.trim_start().starts_with("fix: ") {
            continue;
        }
        skip_fix = line.get(9..19) == Some("space     ") && !line.starts_with("skipped");
        lines.push(if skip_fix {
            "<status> space      <free>".to_string()
        } else {
            line.to_string()
        });
    }
    lines.join("\n") + "\n"
}

#[test]
fn a_ready_home_in_text() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = ready_home(dir.path());
    let assert = sluice(&home)
        .args(["doctor", "--color", "never"])
        .assert()
        .code(0);
    insta::assert_snapshot!(redact(&stdout_text(&assert), &home));
}

#[test]
fn a_ready_home_in_json() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = ready_home(dir.path());
    let assert = sluice(&home).args(["doctor", "--json"]).assert().code(0);
    let stdout = stdout_text(&assert);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("one JSON object");
    assert_eq!(parsed["checks"].as_array().expect("a list").len(), 8);
    assert_eq!(stdout.matches('\n').count(), 1, "one line");
    insta::assert_snapshot!(redact(&stdout, &home));
}

#[test]
fn a_missing_home_in_text_and_in_json() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = dir.path().join("home");
    let text = sluice(&home)
        .args(["doctor", "--color", "never"])
        .assert()
        .code(5);
    insta::assert_snapshot!("missing_text", redact(&stdout_text(&text), &home));
    let json = sluice(&home).args(["doctor", "--json"]).assert().code(5);
    insta::assert_snapshot!("missing_json", redact(&stdout_text(&json), &home));
    assert!(!home.exists(), "doctor created the home");
}

#[test]
fn warnings_alone_exit_0() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = dir.path().join("home");
    drop(sluice_core::Home::open(&home).expect("a home"));
    let assert = sluice(&home)
        .args(["doctor", "--color", "never"])
        .assert()
        .code(0);
    assert!(
        stdout_text(&assert).contains("warning  key"),
        "{}",
        stdout_text(&assert)
    );
}

#[test]
fn a_storage_failure_exits_5() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = dir.path().join("home");
    fs::write(&home, "a file").expect("write");
    let assert = sluice(&home).arg("doctor").assert().code(5);
    assert!(stdout_text(&assert).contains("is not a folder"));
    assert_eq!(
        stderr_text(&assert),
        "",
        "the report is the output, not an error"
    );
}

#[test]
fn a_policy_failure_exits_6() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = ready_home(dir.path());
    fs::write(home.join("VERSION"), "2\n").expect("write");
    sluice(&home).arg("doctor").assert().code(6);
    sluice(&home).args(["doctor", "--json"]).assert().code(6);
}

#[test]
fn a_policy_and_a_storage_failure_together_exit_6() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = ready_home(dir.path());
    fs::write(home.join("VERSION"), "2\n").expect("write");
    fs::OpenOptions::new()
        .write(true)
        .open(home.join("identity.key"))
        .and_then(|key| key.set_len(5))
        .expect("truncate");
    let assert = sluice(&home)
        .args(["doctor", "--color", "never"])
        .assert()
        .code(6);
    let stdout = stdout_text(&assert);
    assert!(
        stdout.contains("failed   layout") && stdout.contains("failed   key"),
        "{stdout}"
    );
}

#[test]
fn colour_follows_the_flag_and_no_color() {
    let dir = tempfile::tempdir().expect("a temporary folder");
    let home = ready_home(dir.path());
    let always = sluice(&home)
        .args(["doctor", "--color", "always"])
        .assert()
        .code(0);
    assert!(stdout_text(&always).contains("\u{1b}[32mok\u{1b}[0m"));
    let never = sluice(&home)
        .args(["doctor", "--color", "never"])
        .assert()
        .code(0);
    assert!(!stdout_text(&never).contains('\u{1b}'));
    let no_color = sluice(&home)
        .env("NO_COLOR", "1")
        .args(["doctor", "--color", "always"])
        .assert()
        .code(0);
    assert!(!stdout_text(&no_color).contains('\u{1b}'));
    let json = sluice(&home)
        .args(["doctor", "--json", "--color", "always"])
        .assert()
        .code(0);
    assert!(
        !stdout_text(&json).contains('\u{1b}'),
        "JSON is never coloured"
    );
}
/// The network checks report what they find and change no setting. Without a relay they say what
/// that rules out, which is more than two hard NATs: across the internet only a router that opens
/// a port or a full-cone NAT lets the other side reach this one. The command still exits as the
/// local checks decide.
#[test]
fn network_checks_report_without_a_relay() {
    let dir = tempfile::tempdir().unwrap();
    let home = ready_home(dir.path());
    let result = sluice(&home)
        .args(["doctor", "--network"])
        .assert()
        .success();
    let text = stdout_text(&result);
    assert!(text.contains("udp"), "{text}");
    assert!(
        text.contains("only a sender whose router opens a port"),
        "{text}"
    );
    assert!(text.contains("run a relay as a meeting point"), "{text}");
    assert!(text.contains("portmap"), "{text}");
    assert!(!text.contains("rendezvous"), "{text}");
}
