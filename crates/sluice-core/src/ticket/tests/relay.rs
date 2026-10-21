//! Rule 7: the one spelling a `relay_url` may have.

use super::super::{Ticket, relay};
use super::common::*;
use crate::Error;

/// A `relay_url` that rule 7 accepts, kept by `Ticket::new` and read back by `decode` unchanged.
const ACCEPTED: [&str; 9] = [
    "https://relay.example/",
    "https://relay.example:3340/",
    "http://relay.example/",
    "http://relay.example:8080/",
    "https://[2001:db8::1]:3340/",
    "https://192.0.2.7:3340/",
    "https://relay.example/path/to",
    "https://relay.example/a%20b",
    "https://relay.example./",
];

/// A `relay_url` rule 7 refuses, with the words of the clause it breaks.
const REFUSED: [(&str, &str); 24] = [
    ("ftp://relay.example/", "scheme https or http"),
    ("mailto:ops@relay.example", "scheme https or http"),
    ("javascript:alert(1)", "scheme https or http"),
    ("relay.example/x", "valid absolute URL"),
    ("/relay", "valid absolute URL"),
    ("https://relay.exa mple/", "valid absolute URL"),
    ("https://", "has no host"),
    ("https:///x", "serialized form"),
    ("https://user@relay.example/", "user name, a password"),
    (
        "https://user:secret@relay.example/",
        "user name, a password",
    ),
    ("https://:secret@relay.example/", "user name, a password"),
    ("https://relay.example/?a=1", "a query or a fragment"),
    ("https://relay.example/?", "a query or a fragment"),
    ("https://relay.example/#top", "a query or a fragment"),
    ("https://relay.example/#", "a query or a fragment"),
    ("HTTPS://relay.example/", "serialized form"),
    ("https://Relay.Example/", "serialized form"),
    ("https://relay.example", "serialized form"),
    ("https://relay.example:443/", "serialized form"),
    ("http://relay.example:80/", "serialized form"),
    ("https://relay.example/a b", "serialized form"),
    ("https://relay.example/\u{e9}", "serialized form"),
    ("https://m\u{fc}nchen.example/", "serialized form"),
    ("https://[2001:DB8::1]/", "serialized form"),
];

fn long_url(len: usize) -> String {
    let start = "https://relay.example/";
    format!("{start}{}", "a".repeat(len - start.len()))
}

#[test]
fn a_relay_url_in_its_one_spelling_is_kept() {
    for relay in ACCEPTED {
        let ticket = Ticket::new(ID, vec![], Some(relay.into()), None, live_body())
            .unwrap_or_else(|e| panic!("{relay}: {e}"));
        assert_eq!(ticket.relay_url(), Some(relay));
        let decoded = Ticket::decode(&ticket.encode()).unwrap();
        assert_eq!(decoded.relay_url(), Some(relay));
    }
}

#[test]
fn every_relay_url_that_breaks_rule_7_is_refused_by_new_and_by_decode() {
    for (relay, clause) in REFUSED {
        let from_new =
            Ticket::new(ID, vec![], Some(relay.into()), None, live_body()).expect_err(relay);
        let Error::Policy(message) = from_new else {
            panic!("{relay}: not a policy error: {from_new:?}");
        };
        assert!(message.contains(clause), "{relay}: {message}");
        // The text came from the caller, so `new` may say what it wanted.
        assert!(
            message.contains("https://relay.example/"),
            "{relay}: {message}"
        );

        let parts = Parts {
            relay: relay_bytes(relay),
            ..Parts::valid()
        };
        let from_decode = refusal_of(&parts.bytes());
        assert!(from_decode.contains(clause), "{relay}: {from_decode}");
    }
}

#[test]
fn a_relay_url_of_2048_bytes_is_kept_and_one_of_2049_is_not() {
    let longest = long_url(2048);
    let ticket = Ticket::new(ID, vec![], Some(longest.clone()), None, live_body()).unwrap();
    assert_eq!(
        Ticket::decode(&ticket.encode()).unwrap().relay_url(),
        Some(&*longest)
    );

    let too_long = long_url(2049);
    assert_eq!(too_long.len(), 2049);
    let err = Ticket::new(ID, vec![], Some(too_long.clone()), None, live_body()).unwrap_err();
    assert!(err.to_string().contains("longer than 2048 bytes"), "{err}");
    let parts = Parts {
        relay: relay_bytes(&too_long),
        ..Parts::valid()
    };
    assert!(refusal_of(&parts.bytes()).contains("longer than 2048 bytes"));
}

#[test]
fn an_empty_relay_url_is_still_the_empty_rule_and_not_rule_7() {
    let err = Ticket::new(ID, vec![], Some(String::new()), None, live_body()).unwrap_err();
    assert!(err.to_string().contains("empty relay_url"), "{err}");
}

#[test]
fn the_check_agrees_with_the_serialization_the_url_crate_gives() {
    // A URL is accepted exactly when parsing and printing it gives the same text, so a fault that
    // the clauses do not name is still caught.
    for relay in ACCEPTED {
        assert_eq!(url::Url::parse(relay).unwrap().as_str(), relay);
        assert_eq!(relay::check(relay), Ok(()));
    }
    assert_eq!(
        relay::check("https://relay.example"),
        Err(relay::Fault::NotSerialized)
    );
}
