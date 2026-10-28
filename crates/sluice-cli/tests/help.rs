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

/// Help is prose for a user: a doc comment that carried a line break or a comment marker into its
/// text prints the source instead, and one flag with a broken doc comment is not something a reader
/// would think to look for.
#[test]
fn no_help_text_carries_the_source_of_a_doc_comment() {
    for args in [
        &["--help"][..],
        &["send", "--help"],
        &["recv", "--help"],
        &["id", "--help"],
        &["doctor", "--help"],
    ] {
        let text = help(args);
        assert!(
            !text.contains("\\n"),
            "{args:?} prints a backslash-n: {text}"
        );
        assert!(
            !text.contains("///"),
            "{args:?} prints a comment marker: {text}"
        );
    }
}

/// Every size and rate flag reads sizes with the one parser, so each one's help has to say what the
/// letters mean: nobody should have to guess between 1000 and 1024 for the flag they are reading.
#[test]
fn every_size_flag_names_the_units() {
    let send = help(&["send", "--help"]);
    assert!(
        send.contains("KiB") && send.contains("1024"),
        "send --limit-rate does not name its units: {send}"
    );
    let recv = help(&["recv", "--help"]);
    assert_eq!(
        recv.matches("KiB").count(),
        3,
        "one of --range, --limit-rate and --max-size does not name its units: {recv}"
    );
}
