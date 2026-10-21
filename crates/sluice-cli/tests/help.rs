//! Snapshots of the help text, so a change to it shows up in review. `--color never` keeps the
//! escape codes out of them.

mod common;

use common::{bare, stdout_text};

fn help(args: &[&str]) -> String {
    let assert = bare()
        .args(args)
        .args(["--color", "never"])
        .assert()
        .success();
    // The line ends of a snapshot are `\n` whatever the platform.
    stdout_text(&assert).replace("\r\n", "\n")
}

#[test]
fn sluice_help() {
    insta::assert_snapshot!(help(&["--help"]));
}

#[test]
fn sluice_id_help() {
    insta::assert_snapshot!(help(&["id", "--help"]));
}
