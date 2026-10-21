//! `sluice id` through the built binary, with a home in a temporary folder.

use std::fs;
use std::path::Path;

use assert_cmd::Command;

/// `sluice` with `SLUICE_HOME` set to `home`. A test never touches the user's real home.
fn sluice(home: &Path) -> Command {
    let mut command = Command::cargo_bin("sluice").unwrap();
    command.env("SLUICE_HOME", home);
    command
}

fn stdout_of(command: &mut Command) -> String {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

fn stderr_of(assert: &assert_cmd::assert::Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

fn is_endpoint_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[test]
fn the_first_run_makes_the_key_and_the_second_prints_the_same_id() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let first = stdout_of(sluice(&home).arg("id"));
    assert!(first.ends_with('\n'));
    assert!(is_endpoint_id(first.trim_end()), "{first:?}");
    assert_eq!(fs::read(home.join("identity.key")).unwrap().len(), 32);
    assert_eq!(stdout_of(sluice(&home).arg("id")), first);
}

#[test]
fn json_prints_one_object_with_the_same_id() {
    let dir = tempfile::tempdir().unwrap();
    let id = stdout_of(sluice(dir.path()).arg("id"));
    let json = stdout_of(sluice(dir.path()).args(["id", "--json"]));
    assert_eq!(json, format!("{{\"endpoint_id\":\"{}\"}}\n", id.trim_end()));
}

#[test]
fn rotate_with_yes_changes_the_id_and_the_next_run_prints_the_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let before = stdout_of(sluice(dir.path()).arg("id"));
    let rotated = stdout_of(sluice(dir.path()).args(["id", "--rotate", "--yes"]));
    assert!(is_endpoint_id(rotated.trim_end()), "{rotated:?}");
    assert_ne!(rotated, before);
    assert_eq!(stdout_of(sluice(dir.path()).arg("id")), rotated);
}

#[test]
fn rotate_without_a_terminal_and_without_yes_exits_6_and_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let before = stdout_of(sluice(dir.path()).arg("id"));
    let assert = sluice(dir.path()).args(["id", "--rotate"]).assert().code(6);
    assert!(stderr_of(&assert).contains("--yes"));
    assert_eq!(stdout_of(sluice(dir.path()).arg("id")), before);
}

#[test]
fn a_refused_rotate_does_not_even_create_the_home() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    sluice(&home).args(["id", "--rotate"]).assert().code(6);
    assert!(!home.exists());
}

#[test]
fn a_home_with_a_newer_layout_exits_6_and_is_not_written_to() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "2\n").unwrap();
    let assert = sluice(dir.path()).arg("id").assert().code(6);
    assert!(stderr_of(&assert).contains('2'));
    let mut names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["VERSION"]);
}

#[test]
fn a_version_file_that_holds_no_version_exits_5() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("VERSION"), "one\n").unwrap();
    sluice(dir.path()).arg("id").assert().code(5);
}

#[cfg(unix)]
#[test]
fn a_key_at_mode_0644_exits_6_and_is_left_as_it_is() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    sluice(dir.path()).arg("id").assert().success();
    let key = dir.path().join("identity.key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    let assert = sluice(dir.path()).arg("id").assert().code(6);
    let stderr = stderr_of(&assert);
    assert!(stderr.contains("chmod 600"));
    assert!(!stderr.contains("  "), "a run of spaces in: {stderr}");
    let mode = fs::metadata(&key).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o644);
}

#[test]
fn yes_without_rotate_is_a_command_line_error_with_code_2() {
    let dir = tempfile::tempdir().unwrap();
    sluice(dir.path()).args(["id", "--yes"]).assert().code(2);
}
