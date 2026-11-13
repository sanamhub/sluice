//! Each point of ADR 0036 that guards the page's listener, tried against a daemon as a process.
//! Everything is on 127.0.0.1.

mod common;

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::Duration;

use common::page::{authorized, get, page, post, raw};

const CSP: &str = "default-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

#[test]
fn the_key_is_required_and_a_near_miss_is_refused() {
    let page = page();
    assert_eq!(get(&page, "/api/v1/version", &[]).status, 401);
    let mut near = page.key.clone();
    near.replace_range(0..1, if near.starts_with('a') { "b" } else { "a" });
    assert_eq!(
        get(
            &page,
            "/api/v1/version",
            &[("Authorization", &format!("Bearer {near}"))]
        )
        .status,
        401
    );
    assert_eq!(
        get(&page, "/api/v1/version", &[("Authorization", &page.key)]).status,
        401
    );
    let right = get(
        &page,
        "/api/v1/version",
        &[("Authorization", &format!("Bearer {}", page.key))],
    );
    assert_eq!(right.status, 200);
    assert_eq!(page.key.len(), 64, "32 bytes in hex");
}

#[test]
fn a_host_that_is_not_the_loopback_address_or_localhost_on_the_port_is_refused() {
    let page = page();
    let key = format!("Bearer {}", page.key);
    for host in [
        "evil.example",
        &format!("evil.example:{}", page.port),
        "127.0.0.1",
        &format!("127.0.0.1:{}", page.port + 1),
    ] {
        let answer = get(
            &page,
            "/api/v1/version",
            &[("Host", host), ("Authorization", &key)],
        );
        assert_eq!(answer.status, 421, "{host}");
        assert_eq!(
            get(&page, "/", &[("Host", host)]).status,
            421,
            "the page too: {host}"
        );
    }
    let localhost = format!("localhost:{}", page.port);
    assert_eq!(
        get(
            &page,
            "/api/v1/version",
            &[("Host", &localhost), ("Authorization", &key)]
        )
        .status,
        200
    );
}

#[test]
fn an_origin_that_is_not_the_pages_own_is_refused() {
    let page = page();
    let key = format!("Bearer {}", page.key);
    let foreign = get(
        &page,
        "/api/v1/version",
        &[("Origin", "https://evil.example"), ("Authorization", &key)],
    );
    assert_eq!(foreign.status, 403);
    let null = get(
        &page,
        "/api/v1/version",
        &[("Origin", "null"), ("Authorization", &key)],
    );
    assert_eq!(null.status, 403);
    let own = format!("http://127.0.0.1:{}", page.port);
    assert_eq!(
        get(
            &page,
            "/api/v1/version",
            &[("Origin", &own), ("Authorization", &key)]
        )
        .status,
        200
    );
}

#[test]
fn no_answer_has_a_cors_header_and_a_preflight_is_not_answered() {
    let page = page();
    let key = format!("Bearer {}", page.key);
    let own = format!("http://127.0.0.1:{}", page.port);
    let mut answers = vec![
        get(&page, "/", &[]),
        get(&page, "/app.js", &[]),
        get(&page, "/api/v1/version", &[("Authorization", &key)]),
        get(&page, "/api/v1/version", &[]),
        get(&page, "/nothing", &[]),
        get(
            &page,
            "/api/v1/version",
            &[("Origin", "https://evil.example"), ("Authorization", &key)],
        ),
    ];
    for origin in ["https://evil.example", own.as_str()] {
        let preflight = format!(
            "OPTIONS /api/v1/send HTTP/1.1\r\nConnection: close\r\nHost: 127.0.0.1:{}\r\nOrigin: {origin}\r\nAccess-Control-Request-Method: POST\r\nAccess-Control-Request-Headers: authorization,content-type\r\n\r\n",
            page.port
        );
        let answer = raw(page.port, &preflight);
        assert!(
            answer.status == 403 || answer.status == 405,
            "{}",
            answer.status
        );
        answers.push(answer);
    }
    for answer in &answers {
        assert!(
            answer.cors().is_empty(),
            "{:?} on a {}",
            answer.cors(),
            answer.status
        );
    }
}

#[test]
fn a_post_has_to_be_json() {
    let page = page();
    let body = r#"{"root":"files","path":""}"#;
    assert_eq!(
        post(&page, "/api/v1/entries", "application/json", body, &[]).status,
        200
    );
    assert_eq!(
        post(
            &page,
            "/api/v1/entries",
            "application/json; charset=utf-8",
            body,
            &[]
        )
        .status,
        200
    );
    for wrong in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data; boundary=x",
        "text/json",
    ] {
        assert_eq!(
            post(&page, "/api/v1/entries", wrong, body, &[]).status,
            415,
            "{wrong}"
        );
    }
}

#[test]
fn a_body_over_64_kib_is_refused_before_it_is_read() {
    let page = page();
    let big = format!(r#"{{"root":"files","path":"{}"}}"#, "a".repeat(70 * 1024));
    assert_eq!(
        post(&page, "/api/v1/entries", "application/json", &big, &[]).status,
        413
    );
    // Only the head is sent, claiming 70 000 bytes: the refusal comes without the body.
    let (a, b) = authorized(&page);
    let head = format!(
        "POST /api/v1/entries HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n{a}: {b}\r\nContent-Type: application/json\r\nContent-Length: 70000\r\n\r\n",
        page.port
    );
    let mut stream = TcpStream::connect(("127.0.0.1", page.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(head.as_bytes()).unwrap();
    let mut first = [0u8; 12];
    stream
        .read_exact(&mut first)
        .expect("an answer without the body");
    assert_eq!(
        &first,
        b"HTTP/1.1 413",
        "{}",
        String::from_utf8_lossy(&first)
    );
}

#[test]
fn the_listener_is_on_loopback_only() {
    let page = page();
    // The address this computer would use to reach another: found without sending a packet.
    let probe = UdpSocket::bind("0.0.0.0:0").unwrap();
    if probe.connect("192.0.2.1:9").is_err() {
        return;
    }
    let ip = probe.local_addr().unwrap().ip();
    if ip.is_loopback() || ip.is_unspecified() || matches!(ip, IpAddr::V6(_)) {
        return;
    }
    let reached =
        TcpStream::connect_timeout(&SocketAddr::new(ip, page.port), Duration::from_secs(2));
    assert!(reached.is_err(), "the page answers on {ip}");
}

#[test]
fn the_page_carries_the_policy_the_decision_states() {
    let page = page();
    let answer = get(&page, "/", &[]);
    assert_eq!(answer.status, 200);
    assert_eq!(answer.header("content-security-policy"), Some(CSP));
    assert_eq!(answer.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(answer.header("referrer-policy"), Some("no-referrer"));
    assert!(
        answer
            .header("content-type")
            .unwrap()
            .starts_with("text/html")
    );
    assert!(answer.body.contains("<title>Sluice</title>"));
    for asset in ["/app.css", "/api.js", "/app.js", "/favicon.svg"] {
        assert_eq!(get(&page, asset, &[]).status, 200, "{asset}");
    }
}
