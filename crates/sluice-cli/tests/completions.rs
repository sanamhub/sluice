//! `sluice completions <shell>` for each shell it supports.

mod common;

use common::{bare, stderr_text, stdout_text};

#[test]
fn every_shell_gets_a_script_that_names_sluice() {
    for shell in ["bash", "zsh", "fish", "powershell", "elvish"] {
        let assert = bare().args(["completions", shell]).assert().success();
        let script = stdout_text(&assert);
        assert!(!script.is_empty(), "{shell}");
        assert!(script.contains("sluice"), "{shell}");
        assert_eq!(stderr_text(&assert), "", "{shell}");
    }
}

#[test]
fn the_script_does_not_change_with_json() {
    let plain = stdout_text(&bare().args(["completions", "bash"]).assert().success());
    let json = stdout_text(
        &bare()
            .args(["--json", "completions", "bash"])
            .assert()
            .success(),
    );
    assert_eq!(plain, json);
}

#[test]
fn an_unknown_shell_is_a_command_line_error() {
    bare().args(["completions", "nushell"]).assert().code(2);
}
