//! The rules of ADR 0020 that hold in every mode: HTTPS stays on, metrics are written out, and the
//! HTTP access URL holds no secret.

use super::m;
use crate::{Mode, check_config};

const RATE: &str = "\n[limits.client.rx]\nbytes_per_second = 2097152\n";
const ALLOW: &str = "[access]\nallowlist = []\n";
const TLS: &str = "[tls]\ncert_mode = \"Manual\"\n";

fn refused(text: &str, mode: Mode, says: &str) {
    let message = check_config(text, mode, None)
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
    assert_eq!(check_config(&on, Mode::Closed, None), Ok(()));
}

#[test]
fn no_tls_at_all_stays_allowed_for_a_relay_behind_a_proxy() {
    assert_eq!(check_config(&m(ALLOW), Mode::Closed, None), Ok(()));
}

#[test]
fn enable_metrics_must_be_written_and_either_value_passes() {
    refused(ALLOW, Mode::Closed, "`enable_metrics` is not set");
    refused(
        &format!("access = \"everyone\"\n{RATE}"),
        Mode::Open,
        "`enable_metrics` is not set",
    );
    for value in ["true", "false"] {
        let text = format!("enable_metrics = {value}\n{ALLOW}");
        assert_eq!(check_config(&text, Mode::Closed, None), Ok(()), "{value}");
        let open = format!("enable_metrics = {value}\naccess = \"everyone\"\n{RATE}");
        assert_eq!(check_config(&open, Mode::Open, None), Ok(()), "{value}");
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
            check_config(&http(url), Mode::Closed, None),
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
        let message = check_config(&http(url), Mode::Closed, None)
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
