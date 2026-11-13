use std::net::SocketAddr;

use super::*;

#[test]
fn a_peer_that_is_not_on_loopback_is_refused() {
    let loopback: [SocketAddr; 2] = [
        "127.0.0.1:5000".parse().unwrap(),
        "[::1]:5000".parse().unwrap(),
    ];
    for peer in loopback {
        assert!(is_loopback_peer(peer), "{peer}");
    }
    let others: [SocketAddr; 4] = [
        "192.168.1.20:5000".parse().unwrap(),
        "10.0.0.7:5000".parse().unwrap(),
        "8.8.8.8:5000".parse().unwrap(),
        "[2001:db8::1]:5000".parse().unwrap(),
    ];
    for peer in others {
        assert!(!is_loopback_peer(peer), "{peer}");
    }
}

#[test]
fn the_host_has_to_be_the_loopback_address_or_localhost_on_the_port() {
    assert!(check_host(Some("127.0.0.1:4242"), 4242).is_ok());
    assert!(check_host(Some("localhost:4242"), 4242).is_ok());
    assert!(check_host(Some("LOCALHOST:4242"), 4242).is_ok());
    for wrong in [
        None,
        Some("127.0.0.1:4243"),
        Some("127.0.0.1"),
        Some("evil.example:4242"),
        Some("rebind.example.com:4242"),
        Some("127.0.0.1.evil.example:4242"),
        Some("[::1]:4242"),
        Some(""),
    ] {
        let refused = check_host(wrong, 4242).unwrap_err();
        assert_eq!(refused.status, 421, "{wrong:?}");
    }
}

#[test]
fn an_origin_that_is_not_the_pages_own_is_refused_and_none_is_allowed() {
    assert!(
        check_origin(None, 4242).is_ok(),
        "a request with no origin is the page's own or a tool"
    );
    assert!(check_origin(Some("http://127.0.0.1:4242"), 4242).is_ok());
    assert!(check_origin(Some("http://localhost:4242"), 4242).is_ok());
    for wrong in [
        "https://evil.example",
        "http://evil.example:4242",
        "http://127.0.0.1:4243",
        "https://127.0.0.1:4242",
        "null",
        "http://127.0.0.1:4242.evil.example",
        "",
    ] {
        let refused = check_origin(Some(wrong), 4242).unwrap_err();
        assert_eq!(refused.status, 403, "{wrong}");
    }
}

#[test]
fn the_key_is_compared_whole_and_a_near_miss_is_refused() {
    let key = "ab".repeat(32);
    assert!(check_key(Some(&format!("Bearer {key}")), &key).is_ok());
    let mut flipped = key.clone();
    flipped.replace_range(63..64, "c");
    let mut shorter = key.clone();
    shorter.pop();
    for wrong in [
        None,
        Some(String::new()),
        Some(key.clone()),
        Some(format!("Basic {key}")),
        Some(format!("Bearer {flipped}")),
        Some(format!("Bearer {shorter}")),
        Some(format!("Bearer {key}x")),
        Some("Bearer ".to_string()),
    ] {
        let refused = check_key(wrong.as_deref(), &key).unwrap_err();
        assert_eq!(refused.status, 401, "{wrong:?}");
    }
}

#[test]
fn the_comparison_looks_at_every_byte_and_says_only_whether_all_matched() {
    assert!(same_bytes(b"", b""));
    assert!(same_bytes(b"abc", b"abc"));
    // The difference is in the first byte, the last, or the middle: the answer is the same.
    assert!(!same_bytes(b"xbc", b"abc"));
    assert!(!same_bytes(b"abx", b"abc"));
    assert!(!same_bytes(b"axc", b"abc"));
    assert!(!same_bytes(b"ab", b"abc"));
    assert!(!same_bytes(b"abcd", b"abc"));
}

#[test]
fn a_post_has_to_be_json() {
    assert!(check_json(Some("application/json")).is_ok());
    assert!(check_json(Some("application/json; charset=utf-8")).is_ok());
    assert!(check_json(Some("Application/JSON")).is_ok());
    for wrong in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
        Some("multipart/form-data; boundary=x"),
        Some("application/jsonp"),
        Some("text/json"),
        Some(""),
    ] {
        let refused = check_json(wrong).unwrap_err();
        assert_eq!(refused.status, 415, "{wrong:?}");
    }
}

#[test]
fn a_body_that_says_it_is_over_64_kib_is_refused_before_it_is_read() {
    assert!(check_length(None).is_ok());
    assert!(check_length(Some("0")).is_ok());
    assert!(check_length(Some(&MAX_BODY_BYTES.to_string())).is_ok());
    let refused = check_length(Some(&(MAX_BODY_BYTES + 1).to_string())).unwrap_err();
    assert_eq!(refused.status, 413);
    assert_eq!(check_length(Some("99999999999")).unwrap_err().status, 413);
    assert_eq!(check_length(Some("many")).unwrap_err().status, 400);
}
