use std::net::SocketAddr;

use super::*;

#[test]
fn the_accept_loop_serves_only_a_loopback_peer() {
    assert!(allowed_peer("127.0.0.1:1".parse::<SocketAddr>().unwrap()));
    assert!(!allowed_peer(
        "192.168.0.9:1".parse::<SocketAddr>().unwrap()
    ));
    assert!(!allowed_peer(
        "[2001:db8::2]:1".parse::<SocketAddr>().unwrap()
    ));
}

#[test]
fn a_key_is_32_random_bytes_as_64_lower_case_hex_and_no_two_are_alike() {
    let (first, second) = (make_key().unwrap(), make_key().unwrap());
    for key in [&first, &second] {
        assert_eq!(key.len(), 64);
        assert!(
            key.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }
    assert_ne!(first, second);
}

#[test]
fn no_answer_carries_a_cors_header_and_every_answer_carries_the_four_it_must() {
    for response in [
        respond(200, "text/html", full("x")),
        error(403, "refused"),
        error(405, "preflight"),
    ] {
        let names: Vec<String> = response
            .headers()
            .keys()
            .map(|k| k.as_str().to_string())
            .collect();
        assert!(
            !names.iter().any(|n| n.starts_with("access-control")),
            "{names:?}"
        );
        for (name, value) in assets::HEADERS {
            assert_eq!(response.headers()[name], value, "{name}");
        }
    }
}

#[test]
fn the_policy_is_exactly_the_one_the_decision_states() {
    assert_eq!(
        assets::CONTENT_SECURITY_POLICY,
        "default-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; \
         base-uri 'none'; form-action 'none'"
    );
}

#[test]
fn the_page_is_embedded_loads_nothing_from_elsewhere_and_has_no_mock() {
    for path in ["/", "/app.css", "/api.js", "/app.js", "/favicon.svg"] {
        assert!(assets::find(path).is_some(), "{path}");
    }
    assert!(assets::find("/mock.js").is_none());
    let (_, html) = assets::find("/").unwrap();
    assert!(!html.contains("mock.js"));
    for (_, _, text) in assets::ASSETS {
        assert!(
            !text.contains("http://") || text.contains("www.w3.org"),
            "an outside address"
        );
        assert!(!text.contains("https://"), "an outside address");
    }
}
