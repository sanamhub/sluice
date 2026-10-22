use crate::{Mode, check_config};

/// An id `sluice id` could print.
const ID: &str = "68852edc24884118b7c22e87f75094674a9b3268ba83b3005e4da869de4ed964";
const RATE: &str = "\n[limits.client.rx]\nbytes_per_second = 2097152\n";

fn refused(text: &str, open: bool, says: &str) -> String {
    let message = check_config(text, open).expect_err("refused").to_string();
    assert!(message.contains(says), "{text:?} open={open}: {message}");
    assert!(!message.contains('\n'), "one line: {message}");
    message
}

#[test]
fn an_unknown_key_is_refused_and_named_at_every_level() {
    refused(
        "enable_relays = true\n",
        false,
        "line 1: unknown key `enable_relays`",
    );
    refused(
        "[access]\nallowlist = []\n[tls]\ncert_mode = \"Manual\"\ncert_dri = \"/c\"\n",
        false,
        "line 5: unknown key `cert_dri`",
    );
    let rx = "[access]\nallowlist = []\n[limits.client.rx]\nbytes_per_second = 1\nburst = 2\n";
    refused(rx, false, "line 5: unknown key `burst`");
    refused(
        "[access]\nallowlist = []\n[limits]\nclient_rx = 1\n",
        false,
        "`client_rx`",
    );
    refused(
        "[access.http]\nurl = \"https://a.example\"\ntoken = \"t\"\n",
        false,
        "`token`",
    );
}

#[test]
fn a_misspelled_access_table_is_refused_and_not_read_as_everyone() {
    let message = refused(
        "[acess]\nallowlist = []\n",
        false,
        "line 1: unknown key `acess`",
    );
    assert!(message.contains("would ignore it"), "{message}");
    // The same typo with --open is the same refusal: it never gets as far as the open rules.
    refused(
        &format!("[acess]\nallowlist = []\n{RATE}"),
        true,
        "unknown key `acess`",
    );
}

#[test]
fn an_unknown_kind_of_access_is_refused() {
    refused(
        "[access]\nallowlst = []\n",
        false,
        "line 2: `allowlst` is not a setting or value",
    );
    refused(
        "access = \"evryone\"\n",
        false,
        "line 1: `access` is not a setting or value",
    );
    let both = format!("[access]\nallowlist = [\"{ID}\"]\ndenylist = []\n");
    refused(&both, false, "line ");
}

#[test]
fn each_closed_mode_starts_without_open_and_is_refused_with_it() {
    let closed = [
        format!("[access]\nallowlist = [\"{ID}\"]\n"),
        "[access.http]\nurl = \"https://auth.example/relay\"\n".to_string(),
        "[access]\nshared_token = [\"a\"]\n".to_string(),
    ];
    for text in closed {
        assert_eq!(check_config(&text, false), Ok(Mode::Closed), "{text}");
        let message = refused(
            &format!("{text}{RATE}"),
            true,
            "--open was given, but `access` is",
        );
        assert!(
            message.contains("which is closed: remove --open"),
            "{message}"
        );
    }
}

#[test]
fn each_open_mode_needs_open_and_a_rate() {
    let open = ["", "access = \"everyone\"\n", "[access]\ndenylist = []\n"];
    let names = [
        "not set, which iroh-relay 1.4.0 reads as everyone",
        "\"everyone\"",
        "a denylist",
    ];
    for (text, name) in open.iter().zip(names) {
        let message = refused(text, false, &format!("`access` is {name}"));
        assert!(message.contains("or start with --open"), "{message}");
        refused(&format!("{text}{RATE}"), false, "or start with --open");
        refused(
            text,
            true,
            "--open needs `limits.client.rx.bytes_per_second`",
        );
        let burst_only = format!("{text}\n[limits.client.rx]\nmax_burst_bytes = 8388608\n");
        refused(
            &burst_only,
            true,
            "--open needs `limits.client.rx.bytes_per_second`",
        );
        assert_eq!(
            check_config(&format!("{text}{RATE}"), true),
            Ok(Mode::Open),
            "{text}"
        );
    }
}

#[test]
fn an_allowlist_id_must_be_64_hex_digits() {
    let list = |ids: &[&str]| format!("[access]\nallowlist = {ids:?}\n");
    assert_eq!(check_config(&list(&[]), false), Ok(Mode::Closed));
    assert_eq!(
        check_config(&list(&[ID, &ID.to_uppercase()]), false),
        Ok(Mode::Closed)
    );
    refused(
        &list(&[&ID[..63]]),
        false,
        "entry 1 of `access.allowlist` is not 64 hex digits",
    );
    refused(
        &list(&[ID, &format!("{ID}0")]),
        false,
        "entry 2 of `access.allowlist`",
    );
    let not_hex = format!("{}g", &ID[..63]);
    refused(&list(&[&not_hex]), false, "entry 1 of `access.allowlist`");
    refused(
        &list(&["<endpoint-id>"]),
        false,
        "paste the id that `sluice id` prints",
    );
}

#[test]
fn text_that_is_not_toml_is_refused_with_its_line() {
    refused("[access]\nallowlist = [\n", false, "is not valid TOML");
    refused(
        "enable_relay = true\nenable_relay = false\n",
        false,
        "not valid TOML at line 2",
    );
}

#[test]
fn a_wrong_type_is_refused_with_its_key() {
    let text = "[access]\nallowlist = []\n[limits.client.rx]\nbytes_per_second = -1\n";
    refused(
        text,
        false,
        "line 4: the value of `bytes_per_second` does not have the type",
    );
    refused(
        "[access]\nallowlist = []\n[tls]\nhttps_bind_addr = 443\n",
        false,
        "`https_bind_addr`",
    );
    refused(
        "[access]\nallowlist = []\n[tls]\nquic_bind_addr = \"x:1\"\n",
        false,
        "line 4",
    );
}

#[test]
fn a_required_key_left_out_is_named() {
    refused(
        "[access]\nallowlist = []\n[tls]\ncert_dir = \"/c\"\n",
        false,
        "line 3: this table needs the key `cert_mode`",
    );
}

#[test]
fn the_shipped_configs_start_only_as_the_readme_says() {
    let relay = super::read(&super::relay_dir().join("relay.toml"));
    let open = super::read(&super::relay_dir().join("open.toml"));
    assert_eq!(check_config(&relay, false), Ok(Mode::Closed));
    refused(
        &relay,
        true,
        "--open was given, but `access` is an allowlist",
    );
    refused(&open, false, "`access` is \"everyone\"");
    assert_eq!(check_config(&open, true), Ok(Mode::Open));
}

#[test]
fn deep_nesting_is_an_error_and_not_a_stack_overflow() {
    let deep = format!("a = {}{}\n", "[".repeat(100_000), "]".repeat(100_000));
    refused(&deep, false, "is not valid TOML");
}
