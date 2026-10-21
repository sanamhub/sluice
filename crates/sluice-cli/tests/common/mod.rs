//! Helpers for the integration tests. Each test file uses some of them.
#![allow(dead_code)]

use std::path::Path;

use assert_cmd::Command;
use assert_cmd::assert::Assert;

/// `sluice` with `SLUICE_HOME` set to `home`, so a test never touches the user's real home.
/// `NO_COLOR` and `RUST_BACKTRACE` are removed, because the developer's shell may set them and
/// both change what is printed.
pub fn sluice(home: &Path) -> Command {
    let mut command = bare();
    command.env("SLUICE_HOME", home);
    command
}

/// `sluice` with no home at all, for commands that never look for one.
pub fn bare() -> Command {
    let mut command = Command::cargo_bin("sluice").unwrap();
    command.env_remove("NO_COLOR").env_remove("RUST_BACKTRACE");
    command
}

pub fn stdout_of(command: &mut Command) -> String {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

pub fn stdout_text(assert: &Assert) -> String {
    String::from_utf8(assert.get_output().stdout.clone()).unwrap()
}

pub fn stderr_text(assert: &Assert) -> String {
    String::from_utf8(assert.get_output().stderr.clone()).unwrap()
}

pub fn is_endpoint_id(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}
