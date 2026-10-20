use std::net::SocketAddr;

use super::super::{Body, Format, Ticket};
use super::common::*;

#[test]
fn the_largest_ticket_that_fits_is_built_and_one_more_address_is_not() {
    let mut addrs = Vec::new();
    let build = |addrs: &[SocketAddr]| Ticket::new(ID, addrs.to_vec(), None, None, live_body());
    loop {
        addrs.push(addr("192.0.2.1:50000"));
        if build(&addrs).is_err() {
            addrs.pop();
            break;
        }
    }
    let largest = build(&addrs).unwrap();
    let text = largest.encode();
    assert!(text.len() <= 4096 && text.len() > 4080, "{}", text.len());
    assert_eq!(Ticket::decode(&text).unwrap(), largest);
    let err = build(&[addrs.clone(), vec![addr("192.0.2.1:50000")]].concat()).unwrap_err();
    assert!(err.to_string().contains("longer than 4096 bytes"));
}

#[test]
fn a_relay_url_of_any_non_empty_text_is_kept() {
    for relay in [
        "r",
        "https://relay.example.com./",
        "\u{e9}\u{65e5}",
        " ",
        "\u{0}",
    ] {
        let ticket = Ticket::new(ID, vec![], Some(relay.into()), None, live_body()).unwrap();
        assert_eq!(
            Ticket::decode(&ticket.encode()).unwrap().relay_url(),
            Some(relay)
        );
    }
}

#[test]
fn debug_shows_the_kind_and_the_endpoint_id_and_nothing_else() {
    let ticket = Ticket::new(
        [0xab; 32],
        vec![addr("192.0.2.77:51515")],
        Some("https://relay.secret.example".into()),
        Some(1_234_567_890_123),
        Body::Static {
            root: [0xcd; 32],
            size: 9_876_543_210,
            format: Format::Raw,
        },
    )
    .unwrap();
    let shown = format!("{ticket:?}");
    assert_eq!(
        shown,
        format!(
            "Ticket {{ kind: \"static\", endpoint_id: \"{}\", rest: <redacted> }}",
            "ab".repeat(32)
        )
    );
    for secret in [
        "cdcd",
        "192.0.2.77",
        "51515",
        "relay.secret",
        "1234567890123",
        "9876543210",
    ] {
        assert!(!shown.contains(secret), "{shown}");
    }
    let live = Ticket::new(
        [0xab; 32],
        vec![],
        None,
        None,
        Body::Live {
            stream_id: [0xef; 16],
            sender_key: [0x12; 32],
        },
    )
    .unwrap();
    let shown = format!("{live:?} {:?}", live.body());
    assert!(shown.contains("kind: \"live\""), "{shown}");
    for secret in ["efef", "1212"] {
        assert!(!shown.contains(secret), "{shown}");
    }
    assert!(!format!("{:#?}", ticket).contains("cdcd"));
}

#[test]
fn a_ticket_expires_at_its_expiry_and_one_without_never_does() {
    let at = |expiry| Ticket::new(ID, vec![], None, expiry, live_body()).unwrap();
    assert!(!at(Some(1000)).is_expired(0));
    assert!(!at(Some(1000)).is_expired(999));
    assert!(at(Some(1000)).is_expired(1000));
    assert!(at(Some(1000)).is_expired(u64::MAX));
    assert!(at(Some(0)).is_expired(0));
    assert!(!at(None).is_expired(u64::MAX));
}

#[test]
fn the_accessors_return_what_was_given() {
    let ticket = Ticket::new(
        ID,
        vec![addr("[::1]:9")],
        Some("r".into()),
        Some(5),
        live_body(),
    )
    .unwrap();
    assert_eq!(ticket.endpoint_id(), &ID);
    assert_eq!(ticket.addrs(), [addr("[::1]:9")]);
    assert_eq!(ticket.relay_url(), Some("r"));
    assert_eq!(ticket.expiry_ms(), Some(5));
    assert_eq!(ticket.body(), &live_body());
}
