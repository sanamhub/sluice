//! End-to-end checks that run the built `sluice` binary.

use assert_cmd::Command;

#[test]
fn version_starts_with_the_binary_name() {
    let assert = Command::cargo_bin("sluice")
        .unwrap()
        .arg("--version")
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.starts_with("sluice "),
        "unexpected output: {stdout:?}"
    );
}
