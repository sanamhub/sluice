//! The rules every command follows, which scripts rely on: where JSON and errors go, the exit
//! codes, colour, a panic and a closed pipe. `sluice id` stands in for every command.

use std::fs;
use std::process::Stdio;

use serde_json::Value;

mod common;

use common::{bare, is_endpoint_id, sluice, stderr_text, stdout_of, stdout_text};

/// A home that a newer sluice made, which every command refuses with exit code 6.
fn newer_home() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "2\n").unwrap();
    dir
}

#[test]
fn json_before_and_after_the_subcommand_gives_the_same_output() {
    let dir = tempfile::tempdir().unwrap();
    let before = stdout_of(sluice(dir.path()).args(["--json", "id"]));
    let after = stdout_of(sluice(dir.path()).args(["id", "--json"]));
    assert_eq!(before, after);
    let parsed: Value = serde_json::from_str(&before).unwrap();
    assert!(is_endpoint_id(parsed["endpoint_id"].as_str().unwrap()));
    assert_eq!(parsed.as_object().unwrap().len(), 1);
    assert_eq!(before.matches('\n').count(), 1);
}

#[test]
fn the_format_does_not_depend_on_where_stdout_goes() {
    // Piped here, which is what `sluice id | clip` is: the bare id, and not JSON.
    let dir = tempfile::tempdir().unwrap();
    let text = stdout_of(sluice(dir.path()).arg("id"));
    assert!(is_endpoint_id(text.trim_end()), "{text:?}");
}

#[test]
fn a_failure_with_json_is_one_json_line_on_stderr_and_nothing_on_stdout() {
    let home = newer_home();
    let assert = sluice(home.path()).args(["id", "--json"]).assert().code(6);
    assert_eq!(stdout_text(&assert), "");
    let stderr = stderr_text(&assert);
    assert_eq!(stderr.matches('\n').count(), 1, "{stderr:?}");
    let parsed: Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(parsed["error"]["kind"], "policy");
    assert!(parsed["error"]["message"].as_str().unwrap().contains('2'));
    assert_eq!(parsed.as_object().unwrap().len(), 1);
}

#[test]
fn the_same_failure_without_json_is_sluice_a_colon_and_the_message() {
    let home = newer_home();
    let assert = sluice(home.path()).arg("id").assert().code(6);
    assert_eq!(stdout_text(&assert), "");
    let stderr = stderr_text(&assert);
    assert!(stderr.starts_with("sluice: policy denied: "), "{stderr:?}");
    assert_eq!(stderr.matches('\n').count(), 1, "{stderr:?}");
}

#[test]
fn rotate_without_yes_is_refused_with_json_too() {
    let dir = tempfile::tempdir().unwrap();
    let assert = sluice(dir.path())
        .args(["--json", "id", "--rotate"])
        .assert()
        .code(6);
    let parsed: Value = serde_json::from_str(&stderr_text(&assert)).unwrap();
    assert_eq!(parsed["error"]["kind"], "policy");
    assert!(
        parsed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--json")
    );
}

#[test]
fn a_bad_command_line_is_code_2_with_or_without_json() {
    for args in [
        &["nonsense"][..],
        &["--json", "nonsense"],
        &["id", "--nope"],
    ] {
        bare().args(args).assert().code(2);
    }
}

/// A reader that has closed its end of the pipe, as `head -c 0` does. The child is started with
/// its stdout piped and the read end dropped at once.
#[test]
fn a_closed_stdout_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    // Make the key first, so the run that follows only has to print.
    sluice(dir.path()).arg("id").assert().success();
    let program = assert_cmd::cargo::cargo_bin("sluice");
    for json in [false, true] {
        let mut command = std::process::Command::new(&program);
        command
            .env("SLUICE_HOME", dir.path())
            .env_remove("NO_COLOR")
            .arg("id")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if json {
            command.arg("--json");
        }
        let mut child = command.spawn().unwrap();
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "json {json}: {:?}", output.status);
        assert!(output.stderr.is_empty(), "json {json}: {:?}", output.stderr);
    }
}

/// The same, as a shell pipeline. `pipefail` makes the status that of sluice and not of `head`.
#[cfg(unix)]
#[test]
fn id_piped_to_head_c_0_exits_0_and_says_nothing() {
    let dir = tempfile::tempdir().unwrap();
    sluice(dir.path()).arg("id").assert().success();
    let program = assert_cmd::cargo::cargo_bin("sluice");
    let output = std::process::Command::new("bash")
        .args(["-c", "set -o pipefail; \"$0\" id | head -c 0"])
        .arg(&program)
        .env("SLUICE_HOME", dir.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    assert!(output.stderr.is_empty(), "{:?}", output.stderr);
}

/// `/dev/full` fails every write with "no space left", which is a failure of storage and not a
/// closed pipe.
#[cfg(target_os = "linux")]
#[test]
fn a_write_that_fails_for_another_reason_is_storage_exit_5() {
    let dir = tempfile::tempdir().unwrap();
    sluice(dir.path()).arg("id").assert().success();
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let output = std::process::Command::new(assert_cmd::cargo::cargo_bin("sluice"))
        .env("SLUICE_HOME", dir.path())
        .arg("id")
        .stdout(full)
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(5));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("sluice: storage error: cannot write"),
        "{stderr}"
    );
}

/// These drive the panic path of the real binary through `SLUICE_TEST_PANIC`, which exists only
/// in a build with debug assertions.
#[cfg(debug_assertions)]
mod panics {
    use serde_json::Value;

    use super::{sluice, stderr_text, stdout_text};

    #[test]
    fn a_panic_is_exit_70_with_the_message_and_no_rust_text() {
        let dir = tempfile::tempdir().unwrap();
        let assert = sluice(dir.path())
            .env("SLUICE_TEST_PANIC", "it broke")
            .arg("id")
            .assert()
            .code(70);
        assert_eq!(stdout_text(&assert), "");
        assert_eq!(stderr_text(&assert), "sluice: internal error: it broke\n");
    }

    #[test]
    fn a_panic_with_json_is_the_bug_error() {
        let dir = tempfile::tempdir().unwrap();
        let assert = sluice(dir.path())
            .env("SLUICE_TEST_PANIC", "say \"no\"\nnow")
            .args(["id", "--json"])
            .assert()
            .code(70);
        assert_eq!(stdout_text(&assert), "");
        let stderr = stderr_text(&assert);
        assert_eq!(stderr.matches('\n').count(), 1, "{stderr:?}");
        let parsed: Value = serde_json::from_str(&stderr).unwrap();
        assert_eq!(parsed["error"]["kind"], "bug");
        assert_eq!(
            parsed["error"]["message"],
            "internal error: say \"no\"\nnow"
        );
    }

    #[test]
    fn rust_backtrace_still_shows_where_it_happened() {
        let dir = tempfile::tempdir().unwrap();
        let assert = sluice(dir.path())
            .env("SLUICE_TEST_PANIC", "it broke")
            .env("RUST_BACKTRACE", "1")
            .arg("id")
            .assert()
            .code(70);
        let stderr = stderr_text(&assert);
        assert!(stderr.contains("panicked at"), "{stderr}");
        assert!(
            stderr.contains("sluice: internal error: it broke"),
            "{stderr}"
        );
    }
}

fn has_escape(text: &str) -> bool {
    text.contains('\x1b')
}

#[test]
fn always_colours_a_parse_error_and_no_color_or_never_do_not() {
    let colour = bare()
        .args(["--color", "always", "nonsense"])
        .assert()
        .code(2);
    assert!(has_escape(&stderr_text(&colour)));

    let no_color = bare()
        .env("NO_COLOR", "1")
        .args(["--color", "always", "nonsense"])
        .assert()
        .code(2);
    assert!(!has_escape(&stderr_text(&no_color)));

    let never = bare()
        .args(["--color", "never", "nonsense"])
        .assert()
        .code(2);
    assert!(!has_escape(&stderr_text(&never)));

    // Not a terminal here, so `auto` is off.
    let auto = bare().arg("nonsense").assert().code(2);
    assert!(!has_escape(&stderr_text(&auto)));
}

#[test]
fn an_empty_no_color_is_ignored_and_the_flag_works_after_the_subcommand() {
    let empty = bare()
        .env("NO_COLOR", "")
        .args(["--color=always", "nonsense"])
        .assert()
        .code(2);
    assert!(has_escape(&stderr_text(&empty)));

    let after = bare()
        .args(["id", "--color", "always", "--nope"])
        .assert()
        .code(2);
    assert!(has_escape(&stderr_text(&after)));
}

#[test]
fn help_goes_to_stdout_with_the_same_colour_choice() {
    let colour = bare()
        .args(["--color", "always", "--help"])
        .assert()
        .success();
    assert!(has_escape(&stdout_text(&colour)));
    let plain = bare()
        .args(["--color", "never", "--help"])
        .assert()
        .success();
    assert!(!has_escape(&stdout_text(&plain)));
}
