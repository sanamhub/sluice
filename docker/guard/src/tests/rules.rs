use super::m;
use crate::{Mode, check_config};

/// An id `sluice id` could print.
const ID: &str = "68852edc24884118b7c22e87f75094674a9b3268ba83b3005e4da869de4ed964";
const RATE: &str = "\n[limits.client.rx]\nbytes_per_second = 2097152\n";

fn refused(text: &str, mode: Mode, says: &str) -> String {
    let message = check_config(text, mode, None, false)
        .expect_err("refused")
        .to_string();
    assert!(message.contains(says), "{text:?} {mode:?}: {message}");
    assert!(!message.contains('\n'), "one line: {message}");
    message
}

#[test]
fn an_unknown_key_is_refused_and_named_at_every_level() {
    refused(
        &m("enable_relays = true\n"),
        Mode::Closed,
        "line 2: unknown key `enable_relays`",
    );
    refused(
        &m("[access]\nallowlist = []\n[tls]\ncert_mode = \"Manual\"\ncert_dri = \"/c\"\n"),
        Mode::Closed,
        "line 6: unknown key `cert_dri`",
    );
    let rx = "[access]\nallowlist = []\n[limits.client.rx]\nbytes_per_second = 1\nburst = 2\n";
    refused(&m(rx), Mode::Closed, "line 6: unknown key `burst`");
    refused(
        &m("[access]\nallowlist = []\n[limits]\nclient_rx = 1\n"),
        Mode::Closed,
        "`client_rx`",
    );
    refused(
        &m("[access]\nallowlist = []\n[limits.client]\ntx = 1\n"),
        Mode::Closed,
        "line 5: unknown key `tx`",
    );
    refused(
        &m("[access.http]\nurl = \"https://a.example\"\ntoken = \"t\"\n"),
        Mode::Closed,
        "`token`",
    );
}

#[test]
fn a_misspelled_access_table_is_refused_and_not_read_as_everyone() {
    let message = refused(
        &m("[acess]\nallowlist = []\n"),
        Mode::Closed,
        "line 2: unknown key `acess`",
    );
    assert!(message.contains("would ignore it"), "{message}");
    // The same typo with --open is the same refusal: it never gets as far as the open rules.
    refused(
        &m(&format!("[acess]\nallowlist = []\n{RATE}")),
        Mode::Open,
        "unknown key `acess`",
    );
}

#[test]
fn an_unknown_kind_of_access_is_refused() {
    refused(
        &m("[access]\nallowlst = []\n"),
        Mode::Closed,
        "line 3: `allowlst` is not a setting or value",
    );
    refused(
        &m("access = \"evryone\"\n"),
        Mode::Closed,
        "line 2: `access` is not a setting or value",
    );
    let both = m(&format!(
        "[access]\nallowlist = [\"{ID}\"]\ndenylist = []\n"
    ));
    refused(&both, Mode::Closed, "line ");
}

#[test]
fn each_closed_mode_starts_without_open_and_is_refused_with_it() {
    let closed = [
        format!("[access]\nallowlist = [\"{ID}\"]\n"),
        "[access.http]\nurl = \"https://auth.example/relay\"\n".to_string(),
        "[access]\nshared_token = [\"a\"]\n".to_string(),
    ];
    for text in closed {
        assert_eq!(
            check_config(&m(&text), Mode::Closed, None, false),
            Ok(()),
            "{text}"
        );
        let message = refused(
            &m(&format!("{text}{RATE}")),
            Mode::Open,
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
        "not set, which iroh-relay 1.4.1 reads as everyone",
        "\"everyone\"",
        "a denylist",
    ];
    for (text, name) in open.iter().zip(names) {
        let message = refused(&m(text), Mode::Closed, &format!("`access` is {name}"));
        assert!(message.contains("or start with --open"), "{message}");
        refused(
            &m(&format!("{text}{RATE}")),
            Mode::Closed,
            "or start with --open",
        );
        refused(
            &m(text),
            Mode::Open,
            "--open needs `limits.client.rx.bytes_per_second`",
        );
        let burst_only = m(&format!(
            "{text}\n[limits.client.rx]\nmax_burst_bytes = 8388608\n"
        ));
        refused(
            &burst_only,
            Mode::Open,
            "--open needs `limits.client.rx.bytes_per_second`",
        );
        assert_eq!(
            check_config(&m(&format!("{text}{RATE}")), Mode::Open, None, false),
            Ok(()),
            "{text}"
        );
    }
}

/// 64 lower-case hex digits that are not an ed25519 point: y = 2, little-endian, and
/// (y² - 1) / (dy² + 1) has no square root mod 2^255 - 19, worked out with Python's
/// exact integers.
const NOT_A_KEY: &str = "0200000000000000000000000000000000000000000000000000000000000000";

#[test]
fn an_id_must_be_64_lower_case_hex_digits_of_a_key_in_either_list() {
    for (list, mode, rate) in [
        ("allowlist", Mode::Closed, ""),
        ("denylist", Mode::Open, RATE),
    ] {
        let ids = |ids: &[&str]| m(&format!("[access]\n{list} = {ids:?}\n{rate}"));
        assert_eq!(check_config(&ids(&[]), mode, None, false), Ok(()));
        assert_eq!(check_config(&ids(&[ID]), mode, None, false), Ok(()));
        let entry = |n: usize| format!("entry {n} of `access.{list}`");
        let short = format!("{} is not 64 lower-case hex digits", entry(1));
        refused(&ids(&[&ID[..63]]), mode, &short);
        refused(&ids(&[ID, &format!("{ID}0")]), mode, &entry(2));
        refused(&ids(&[&format!("{}g", &ID[..63])]), mode, &entry(1));
        let upper = format!("{} has upper-case hex digits", entry(1));
        refused(&ids(&[&ID.to_uppercase()]), mode, &upper);
        refused(&ids(&[ID, &ID.to_uppercase()]), mode, &entry(2));
        let not_a_key = format!(
            "{} is 64 hex digits but not an ed25519 public key",
            entry(1)
        );
        refused(&ids(&[NOT_A_KEY]), mode, &not_a_key);
        for id in ["<endpoint-id>", NOT_A_KEY, &ID.to_uppercase()] {
            refused(&ids(&[id]), mode, "paste the id that `sluice id` prints");
        }
    }
}

#[test]
fn text_that_is_not_toml_is_refused_with_its_line() {
    refused(
        &m("[access]\nallowlist = [\n"),
        Mode::Closed,
        "is not valid TOML",
    );
    refused(
        "enable_relay = true\nenable_relay = false\n",
        Mode::Closed,
        "not valid TOML at line 2",
    );
}

#[test]
fn a_wrong_type_is_refused_with_its_key() {
    let text = "[access]\nallowlist = []\n[limits.client.rx]\nbytes_per_second = -1\n";
    refused(
        &m(text),
        Mode::Closed,
        "line 5: the value of `bytes_per_second` does not have the type",
    );
    refused(
        &m("[access]\nallowlist = []\n[tls]\nhttps_bind_addr = 443\n"),
        Mode::Closed,
        "`https_bind_addr`",
    );
    refused(
        &m("[access]\nallowlist = []\n[tls]\nquic_bind_addr = \"x:1\"\n"),
        Mode::Closed,
        "line 5",
    );
}

#[test]
fn a_required_key_left_out_is_named() {
    refused(
        &m("[access]\nallowlist = []\n[tls]\ncert_dir = \"/c\"\n"),
        Mode::Closed,
        "line 4: this table needs the key `cert_mode`",
    );
}

#[test]
fn the_shipped_configs_start_only_as_the_readme_says() {
    let relay = super::read(&super::relay_dir().join("relay.toml"));
    let open = super::read(&super::relay_dir().join("open.toml"));
    assert_eq!(check_config(&relay, Mode::Closed, None, false), Ok(()));
    refused(
        &relay,
        Mode::Open,
        "--open was given, but `access` is an allowlist",
    );
    refused(&open, Mode::Closed, "`access` is \"everyone\"");
    assert_eq!(check_config(&open, Mode::Open, None, false), Ok(()));
}

#[test]
fn deep_nesting_is_an_error_and_not_a_stack_overflow() {
    let deep = format!("a = {}{}\n", "[".repeat(100_000), "]".repeat(100_000));
    refused(&deep, Mode::Closed, "is not valid TOML");
}
