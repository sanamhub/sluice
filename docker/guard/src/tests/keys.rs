//! What a refusal may print of a key the guard does not know, of an argument and of a path: a key
//! only when it is 1 to 32 lower-case ASCII letters or `_`, an argument only when it is `--` and
//! 1 to 32 lower-case letters or `-`, and every control character escaped.

use std::ffi::OsString;
use std::path::Path;

use super::m;
use crate::{Args, Mode, check_config, open_config};

fn refusal(text: &str, mode: Mode) -> String {
    let message = check_config(text, mode, None, false)
        .expect_err("refused")
        .to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    message
}

#[test]
fn a_plain_unknown_key_is_named_in_every_mode() {
    for mode in [Mode::Closed, Mode::Open] {
        let message = refusal(&m("enable_relays = true\n"), mode);
        assert!(
            message.contains("line 2: unknown key `enable_relays`"),
            "{message}"
        );
        let longest = "k".repeat(32);
        let message = refusal(&m(&format!("{longest} = 1\n")), mode);
        assert!(message.contains(&format!("`{longest}`")), "{message}");
    }
}

#[test]
fn a_token_used_as_a_key_is_not_printed() {
    let base64 = "c2VjcmV0LXRva2Vu+/=";
    // What a person pastes: a 64-digit hex key, and a base64url one, each valid as a bare TOML key.
    let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    let base64url = "c2VjcmV0LXRva2Vu-_Tg9Zk";
    let long = "a".repeat(33);
    let dotted = "s3cr3t.T0KEN";
    for (key, line) in [
        (format!("\"{base64}\""), 2),
        (hex.to_string(), 2),
        (base64url.to_string(), 2),
        (long.clone(), 2),
        (format!("\"{dotted}\""), 2),
        (format!("[tls]\ncert_mode = \"Manual\"\n\"{base64}\""), 4),
    ] {
        for mode in [Mode::Closed, Mode::Open] {
            let message = refusal(&m(&format!("{key} = 1\n")), mode);
            assert!(
                message.contains(&format!("line {line}: a key: ")),
                "{message}"
            );
            for part in [base64, hex, base64url, &long, "s3cr3t", "T0KEN"] {
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

/// A letter-only token is the one thing the rule cannot tell from a setting, so it is printed. A
/// digit, a capital or a `-` in a key is enough to keep it out, and every iroh-relay 1.4.1 key
/// has none.
#[test]
fn a_key_is_printed_only_when_it_is_lower_case_letters_and_underscores() {
    for key in ["a1", "A", "a-b", "k0", "Enable_Relay"] {
        let message = refusal(&m(&format!("{key} = 1\n")), Mode::Closed);
        assert!(message.contains("line 2: a key: "), "{key}: {message}");
    }
    let message = refusal(&m("a_b = 1\n"), Mode::Closed);
    assert!(message.contains("unknown key `a_b`"), "{message}");
}

/// A path with a control character in it is not printed at all, and the message is still one line:
/// the value of `--config-path` has to look like a path to be named.
#[test]
fn a_path_with_control_characters_is_not_printed() {
    let path = Path::new("/no/such/re\nlay\u{1b}[31m.toml");
    let message = open_config(path).unwrap_err().to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(
        message.contains("the config file given to --config-path"),
        "{message}"
    );
    assert!(!message.contains("re\nlay"), "{message:?}");
    let args = [
        OsString::from("--x\n\u{1b}[2J"),
        OsString::from("--config-path=a"),
    ];
    let message = Args::parse(&args).unwrap_err().to_string();
    assert!(!message.chars().any(char::is_control), "{message:?}");
    assert!(!message.contains("--x"), "{message}");
    assert!(message.contains("an argument at position 1"), "{message}");
}
