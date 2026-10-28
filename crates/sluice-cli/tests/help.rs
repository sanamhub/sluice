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

#[test]
fn sluice_doctor_help() {
    insta::assert_snapshot!(help(&["doctor", "--help"]));
}

/// What a received file's mode becomes is a rule a user has to be able to read before they rely on
/// it, so `recv --help` says it.
#[test]
fn recv_help_says_which_mode_a_received_file_keeps() {
    let text = help(&["recv", "--help"]);
    assert!(text.contains("owner read and write"), "{text}");
}
