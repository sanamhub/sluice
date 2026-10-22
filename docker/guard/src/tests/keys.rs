//! What a refusal may print of a key the guard does not know, and of a path: a key only when it is
//! 1 to 64 ASCII letters, digits, `_` or `-`, and every control character escaped.

use std::ffi::OsString;
use std::path::Path;

use super::m;
use crate::{Args, Mode, check_config, open_config};

fn refusal(text: &str, mode: Mode) -> String {
    let message = check_config(text, mode, None)
        .expect_err("refused")
        .to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    message
}

#[test]
fn a_plain_unknown_key_is_named_in_every_mode() {
    for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
        let message = refusal(&m("enable_relays = true\n"), mode);
        assert!(
            message.contains("line 2: unknown key `enable_relays`"),
            "{message}"
        );
        let longest = "k".repeat(64);
        let message = refusal(&m(&format!("{longest} = 1\n")), mode);
        assert!(message.contains(&format!("`{longest}`")), "{message}");
    }
}

#[test]
fn a_token_used_as_a_key_is_not_printed() {
    let base64 = "c2VjcmV0LXRva2Vu+/=";
    let long = "a".repeat(65);
    let dotted = "s3cr3t.T0KEN";
    for (key, line) in [
        (format!("\"{base64}\""), 2),
        (long.clone(), 2),
        (format!("\"{dotted}\""), 2),
        (format!("[tls]\ncert_mode = \"Manual\"\n\"{base64}\""), 4),
    ] {
        for mode in [Mode::Closed, Mode::Open, Mode::Rendezvous] {
            let message = refusal(&m(&format!("{key} = 1\n")), mode);
            assert!(
                message.contains(&format!("line {line}: a key: ")),
                "{message}"
            );
            for part in [base64, &long, "s3cr3t", "T0KEN"] {
                assert!(!message.contains(part), "{key}: {message}");
            }
        }
    }
}

#[test]
fn a_key_with_a_newline_or_an_escape_code_is_not_printed() {
    for key in [
        "\"a\\nb\"",
        "\"\\u001b[31mred\"",
        "\"a\\rb\"",
        "\"tab\\tbed\"",
    ] {
        let message = refusal(&m(&format!("{key} = 1\n")), Mode::Closed);
        assert!(message.contains("line 2: a key: "), "{key}: {message}");
        assert!(
            !message.contains("red") && !message.contains("bed"),
            "{message}"
        );
    }
}

#[test]
fn a_key_with_a_backtick_is_not_printed_in_part() {
    let message = refusal(&m("\"ab`cd\" = 1\n"), Mode::Closed);
    assert!(message.contains("line 2: a key: "), "{message}");
    assert!(!message.contains("ab"), "{message}");
}

#[test]
fn a_path_or_an_argument_with_control_characters_is_escaped() {
    let path = Path::new("/no/such/re\nlay\u{1b}[31m.toml");
    let message = open_config(path).unwrap_err().to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(message.contains("re\\nlay\\u{1b}[31m.toml"), "{message}");
    let args = [
        OsString::from("--x\n\u{1b}[2J"),
        OsString::from("--config-path=a"),
    ];
    let message = Args::parse(&args).unwrap_err().to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(message.contains("`--x\\n\\u{1b}[2J`"), "{message}");
}
