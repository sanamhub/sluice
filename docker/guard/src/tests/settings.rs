//! The rules of ADR 0020 that hold in every mode: HTTPS stays on, metrics are written out, and the
//! HTTP access URL holds no secret.

use super::m;
use crate::{Mode, check_config};

const RATE: &str = "\n[limits.client.rx]\nbytes_per_second = 2097152\n";
const ALLOW: &str = "[access]\nallowlist = []\n";
const TLS: &str = "[tls]\ncert_mode = \"Manual\"\n";

fn refused(text: &str, mode: Mode, says: &str) {
    let message = check_config(text, mode, None, false)
        .expect_err("refused")
        .to_string();
    assert!(message.contains(says), "{text:?} {mode:?}: {message}");
}

#[test]
fn dangerous_http_only_is_refused_closed_or_open() {
    let off = format!("{TLS}dangerous_http_only = true\n");
    refused(
        &m(&format!("{ALLOW}{off}")),
        Mode::Closed,
        "`dangerous_http_only` is true",
    );
    let open = m(&format!("access = \"everyone\"\n{off}{RATE}"));
    refused(&open, Mode::Open, "`dangerous_http_only` is true");
    // Written out as false it is the default, and passes.
    let on = m(&format!("{ALLOW}{TLS}dangerous_http_only = false\n"));
    assert_eq!(check_config(&on, Mode::Closed, None, false), Ok(()));
}

#[test]
fn a_config_with_no_tls_needs_the_behind_proxy_flag_and_one_with_tls_refuses_it() {
    let plain = format!("enable_metrics = false\n{ALLOW}");
    let message = check_config(&plain, Mode::Closed, None, false)
        .expect_err("no tls without the flag")
        .to_string();
    assert!(message.contains("there is no [tls] table"), "{message}");
    assert!(message.contains("clear text"), "{message}");
    assert_eq!(check_config(&plain, Mode::Closed, None, true), Ok(()));
    // Open mode is the same rule.
    let open = format!("enable_metrics = false\naccess = \"everyone\"\n{RATE}");
    assert!(check_config(&open, Mode::Open, None, false).is_err());
    assert_eq!(check_config(&open, Mode::Open, None, true), Ok(()));
    // A config that ends TLS itself has no use for the flag, and the two must agree.
    let message = check_config(&m(ALLOW), Mode::Closed, None, true)
        .expect_err("the flag with tls")
        .to_string();
    assert!(message.contains("--behind-proxy was given"), "{message}");
    assert!(message.contains("ends TLS itself"), "{message}");
    let open_tls = m(&format!("access = \"everyone\"\n{RATE}"));
    let message = check_config(&open_tls, Mode::Open, None, true)
        .expect_err("the flag with tls")
        .to_string();
    assert!(message.contains("--behind-proxy was given"), "{message}");
}

#[test]
fn enable_metrics_must_be_written_and_either_value_passes() {
    refused(
        &format!("{ALLOW}{TLS}"),
        Mode::Closed,
        "`enable_metrics` is not set",
    );
    refused(
        &format!("access = \"everyone\"\n{RATE}{TLS}"),
        Mode::Open,
        "`enable_metrics` is not set",
    );
    for value in ["true", "false"] {
        let text = format!("enable_metrics = {value}\n{ALLOW}{TLS}");
        assert_eq!(
            check_config(&text, Mode::Closed, None, false),
            Ok(()),
            "{value}"
        );
        let open = format!("enable_metrics = {value}\naccess = \"everyone\"\n{RATE}{TLS}");
        assert_eq!(
            check_config(&open, Mode::Open, None, false),
            Ok(()),
            "{value}"
        );
    }
}

fn http(url: &str) -> String {
    m(&format!(
        "[access.http]\nurl = \"{url}\"\nbearer_token = \"t\"\n"
    ))
}

#[test]
fn an_http_url_without_a_secret_in_it_passes() {
    for url in [
        "https://auth.example/relay",
        "http://10.0.0.1:8080/check",
        "https://auth.example",
    ] {
        assert_eq!(
            check_config(&http(url), Mode::Closed, None, false),
            Ok(()),
            "{url}"
        );
    }
}

#[test]
fn an_http_url_with_a_secret_in_it_is_refused_and_points_at_bearer_token() {
    let cases = [
        (
            "https://user@auth.example/relay",
            "holds a user name or password",
        ),
        (
            "https://user:pw@auth.example/relay",
            "holds a user name or password",
        ),
        (
            "https://:pw@auth.example/relay",
            "holds a user name or password",
        ),
        ("https://auth.example/relay?key=pw", "holds a query"),
        ("https://auth.example/relay?", "holds a query"),
        ("https://auth.example/relay#pw", "holds a fragment"),
        ("/relay", "is not an absolute URL"),
        ("auth.example/relay", "is not an absolute URL"),
        ("ftp://auth.example/relay", "is not an http or https URL"),
        ("file:///etc/passwd", "is not an http or https URL"),
    ];
    for (url, why) in cases {
        let message = check_config(&http(url), Mode::Closed, None, false)
            .expect_err(url)
            .to_string();
        assert!(
            message.contains(&format!("`access.http.url` {why}")),
            "{url}: {message}"
        );
        assert!(message.contains("`bearer_token`"), "{message}");
        for part in [
            "user@",
            "user:",
            ":pw",
            "key=pw",
            "#pw",
            "auth.example",
            "passwd",
        ] {
            assert!(!message.contains(part), "{url}: {part} in {message}");
        }
    }
}
