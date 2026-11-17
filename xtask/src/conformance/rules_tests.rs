//! The two vector kinds that check a rule rather than a message: the ALPN registration order and
//! the provider's event mask. Their failure lines must name what disagreed, so a stale vector is
//! readable.

use super::check;
use super::tests::TempDir;

#[test]
fn an_access_case_must_match_the_mask_the_provider_registers() {
    let dir = TempDir::new("access-wrong");
    dir.write(
        "access/x.json",
        r#"{ "description": "d", "cases": [
            { "event": "push", "mode": "disabled" },
            { "event": "get", "mode": "intercept" },
            { "event": "throttle", "mode": "intercept", "mode_when_limited": "intercept" } ] }"#,
    );
    let report = check(dir.path()).expect("run");
    // Two cases disagree plus one line for the events the vector leaves out of the mask.
    assert_eq!(report.failed, 3, "{:?}", report.lines);
    assert_eq!(report.passed_in("access"), 1);
    let text = report.lines.join("\n");
    assert!(
        text.contains(r#"FAIL "get" without a rate limit is intercept_log"#),
        "{text}"
    );
    assert!(text.contains("the vector names"), "{text}");
}

#[test]
fn an_alpn_case_must_match_the_registration_order_and_what_is_served() {
    let cases = [
        (0, "/iroh-bytes/4", true),
        (1, "sluice/1", true),
        (2, "/sluice/pair/1", true),
        (3, "/sluice/deliver/1", true),
        (4, "/sluice/control/1", false),
    ];
    let body: Vec<String> = cases
        .iter()
        .map(|(index, alpn, served)| {
            format!(r#"{{ "index": {index}, "alpn": "{alpn}", "served": {served} }}"#)
        })
        .collect();
    let wrong = body
        .join(", ")
        .replace(r#""alpn": "sluice/1""#, r#""alpn": "sluice/2""#);
    let dir = TempDir::new("alpn-wrong");
    dir.write(
        "alpn/x.json",
        &format!(r#"{{ "description": "d", "cases": [ {wrong} ] }}"#),
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.failed, 1, "{:?}", report.lines);
    assert_eq!(report.passed_in("alpn"), 4);
    let text = report.lines.join("\n");
    assert!(
        text.contains(r#"FAIL position 1 is "sluice/1", the vector says "sluice/2""#),
        "{text}"
    );

    let served = body.join(", ").replace(
        r#"{ "index": 2, "alpn": "/sluice/pair/1", "served": true }"#,
        r#"{ "index": 2, "alpn": "/sluice/pair/1", "served": false }"#,
    );
    let dir = TempDir::new("alpn-served");
    dir.write(
        "alpn/x.json",
        &format!(r#"{{ "description": "d", "cases": [ {served} ] }}"#),
    );
    let report = check(dir.path()).expect("run");
    assert_eq!(report.failed, 1, "{:?}", report.lines);
    assert!(
        report.lines.join("\n").contains("is served: true"),
        "{:?}",
        report.lines
    );
}
