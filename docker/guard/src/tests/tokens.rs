//! Shared tokens and `IROH_RELAY_ACCESS_TOKEN`: iroh-relay 1.4.1 refuses an empty list or an empty
//! token at start, and a set variable replaces the whole list (main.rs:236-244).

use super::m;
use crate::{Mode, check_config};

const RATE: &str = "\n[limits.client.rx]\nbytes_per_second = 2097152\n";

fn tokens(list: &str) -> String {
    m(&format!("[access]\nshared_token = {list}\n"))
}

fn refused(text: &str, mode: Mode, token: Option<&str>, says: &str) {
    let message = check_config(text, mode, token, false)
        .expect_err("refused")
        .to_string();
    assert!(message.contains(says), "{text:?} {mode:?}: {message}");
    assert!(!message.contains('\n'), "one line: {message}");
}

#[test]
fn an_empty_list_or_an_empty_token_is_refused_without_the_variable() {
    let empty = "`access.shared_token` is empty";
    refused(&tokens("[]"), Mode::Closed, None, empty);
    refused(
        &tokens("[\"\"]"),
        Mode::Closed,
        None,
        "entry 1 of `access.shared_token` is empty",
    );
    refused(
        &tokens("[\"a\", \"\"]"),
        Mode::Closed,
        None,
        "entry 2 of `access.shared_token` is empty",
    );
    assert_eq!(
        check_config(&tokens("[\"a\", \"b\"]"), Mode::Closed, None, false),
        Ok(())
    );
}

#[test]
fn a_set_variable_replaces_the_list_so_an_empty_one_passes() {
    for list in ["[]", "[\"\"]", "[\"a\", \"\"]", "[\"a\"]"] {
        assert_eq!(
            check_config(&tokens(list), Mode::Closed, Some("t"), false),
            Ok(()),
            "{list}"
        );
    }
}

#[test]
fn an_empty_variable_is_refused_in_every_mode_and_whatever_the_file_says() {
    let says = "IROH_RELAY_ACCESS_TOKEN is set but empty";
    let closed = m("[access]\nallowlist = []\n");
    let open = m(&format!("access = \"everyone\"\n{RATE}"));
    let rendezvous = "enable_relay = false\nenable_metrics = false\n";
    refused(&closed, Mode::Closed, Some(""), says);
    refused(&tokens("[\"a\"]"), Mode::Closed, Some(""), says);
    refused(&open, Mode::Open, Some(""), says);
    refused(rendezvous, Mode::Rendezvous, Some(""), says);
    // Even a file that is not TOML: the variable is wrong whatever it holds.
    refused("[access", Mode::Closed, Some(""), says);
    for (text, mode) in [
        (closed.as_str(), Mode::Closed),
        (open.as_str(), Mode::Open),
        (rendezvous, Mode::Rendezvous),
    ] {
        assert_eq!(
            check_config(text, mode, Some("t"), false),
            Ok(()),
            "{mode:?}"
        );
    }
}

#[test]
fn shared_tokens_are_closed_and_not_a_rendezvous_setting() {
    refused(
        &format!("{}{RATE}", tokens("[\"a\"]")),
        Mode::Open,
        None,
        "--open was given, but `access` is a list of shared tokens",
    );
    refused(
        &format!("enable_relay = false\n{}", tokens("[\"a\"]")),
        Mode::Rendezvous,
        None,
        "`access` is set",
    );
}
