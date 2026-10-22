//! `SPEC.md` section 4, "Rejected tickets": the error says which rule failed and nothing of the
//! text. Each case here breaks one decode rule, and the test looks for any run of 6 characters of
//! the input in the error.

use super::super::Ticket;
use super::common::*;
use crate::Error;

/// The length of a run of the input that must not turn up in the error. Short enough that
/// quoting a piece of a ticket is caught, and long enough that a word of the message the input
/// happens to share is not.
const RUN: usize = 6;

/// The first run of `RUN` characters of `source` that `message` holds, in any case.
fn shared_run(message: &str, source: &str) -> Option<String> {
    let message = message.to_lowercase();
    let chars: Vec<char> = source.to_lowercase().chars().collect();
    chars
        .windows(RUN)
        .map(|run| run.iter().collect::<String>())
        .find(|run| message.contains(run.as_str()))
}

/// Bytes of a ticket with a key that is not one repeated byte, so a leaked piece of it could not
/// pass for the pattern. These 32 bytes are no point, so a ticket that breaks no other rule
/// breaks rule 8, and rule 8 is checked last.
fn varied(mut bytes: Vec<u8>) -> Vec<u8> {
    for (i, byte) in bytes.iter_mut().enumerate().skip(2).take(32) {
        *byte = (i as u8).wrapping_mul(37).wrapping_add(11);
    }
    bytes
}

/// Text of the given length from the base32 alphabet, with no run of 6 that repeats.
fn filler(len: usize) -> String {
    let alphabet = b"0123456789abcdefghjkmnpqrstvwxyz";
    let mut state = 12_345_u32;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            alphabet[(state >> 16) as usize % alphabet.len()] as char
        })
        .collect()
}

/// A ticket text that breaks `rule` of `SPEC.md` section 4, and the relay text inside it, if the
/// fault is in that.
struct Case {
    rule: u8,
    ticket: String,
    relay: Option<&'static str>,
}

fn case(rule: u8, ticket: String) -> Case {
    Case {
        rule,
        ticket,
        relay: None,
    }
}

fn from_parts(rule: u8, parts: &Parts) -> Case {
    case(rule, text(&varied(parts.bytes())))
}

fn cases() -> Vec<Case> {
    let good = text(&varied(Parts::valid().bytes()));
    let good_body = &good[7..];
    let mut cases = vec![
        // Rule 1.
        case(1, format!("{good}{}", filler(4097 - good.len()))),
        // Rule 2: the prefix, a character, the length and the spare bits.
        case(2, format!("sluice2{good_body}")),
        case(2, format!("{}u{}", &good[..40], &good[40..])),
        case(2, format!("{good}000")),
        case(2, {
            let mut bad = good.clone();
            bad.pop();
            bad.push('z');
            bad
        }),
    ];
    // Rule 3: the version, the kind and the format, each 2.
    let mut bad_format = Parts::valid();
    *bad_format.body.last_mut().unwrap() = 2;
    for parts in [
        Parts {
            version: 2,
            ..Parts::valid()
        },
        Parts {
            kind: 2,
            ..Parts::valid()
        },
        bad_format,
    ] {
        cases.push(from_parts(3, &parts));
    }
    // Rule 4.
    let mut after = varied(Parts::valid().bytes());
    after.extend([9, 9, 9]);
    cases.push(case(4, text(&after)));
    let valid = varied(Parts::valid().bytes());
    cases.push(case(4, text(&valid[..valid.len() - 5])));
    // Rule 5.
    let one_addr = |tag| vec![1, tag, 192, 0, 2, 1, 80];
    for parts in [
        Parts {
            relay: vec![2],
            ..Parts::valid()
        },
        Parts {
            addrs: one_addr(2),
            ..Parts::valid()
        },
        Parts {
            relay: vec![1, 2, 0xc3, 0x28],
            ..Parts::valid()
        },
        Parts {
            expiry: vec![1, 0x81, 0x80, 0x00],
            ..Parts::valid()
        },
    ] {
        cases.push(from_parts(5, &parts));
    }
    // Rule 6.
    cases.push(from_parts(
        6,
        &Parts {
            relay: vec![1, 0],
            ..Parts::valid()
        },
    ));
    // Rule 8: the key `varied` writes.
    cases.push(case(8, good.clone()));
    cases
}

/// Rule 7 cases, which carry the relay text so the test can look for it in the error as well.
fn relay_cases() -> Vec<Case> {
    let faults = [
        "ftp://relay.example/secret-path",
        "relay.example/secret-path",
        "https://user:hunter2@relay.example/",
        "https://relay.example/?token=hunter2",
        "https://relay.example/#hunter2",
        "HTTPS://Relay.Example/secret-path",
        "https://relay.example/secret path",
    ];
    faults
        .into_iter()
        .map(|relay| {
            let parts = Parts {
                relay: relay_bytes(relay),
                ..Parts::valid()
            };
            Case {
                rule: 7,
                ticket: text(&varied(parts.bytes())),
                relay: Some(relay),
            }
        })
        .collect()
}

#[test]
fn no_rule_of_the_decoder_puts_the_input_in_its_error() {
    let mut cases = cases();
    cases.extend(relay_cases());
    let mut rules: Vec<u8> = cases.iter().map(|c| c.rule).collect();
    rules.sort_unstable();
    rules.dedup();
    assert_eq!(rules, [1, 2, 3, 4, 5, 6, 7, 8], "every rule has a case");

    for Case {
        rule,
        ticket,
        relay,
    } in cases
    {
        let err = Ticket::decode(&ticket).expect_err("the ticket breaks a rule");
        let Error::Policy(message) = &err else {
            panic!("rule {rule}: not a policy error: {err:?}");
        };
        let after_prefix: String = ticket.chars().skip(7).collect();
        assert_eq!(
            shared_run(&err.to_string(), &after_prefix),
            None,
            "rule {rule}: {message}"
        );
        if let Some(relay) = relay {
            assert_eq!(shared_run(&err.to_string(), relay), None, "{message}");
        }
    }
}

#[test]
fn the_check_does_find_a_run_that_is_there() {
    assert_eq!(
        shared_run("a 248h24 b", "x248h24y").as_deref(),
        Some("248h24")
    );
    assert_eq!(
        shared_run("a 248H24 b", "x248h24y").as_deref(),
        Some("248h24")
    );
    assert_eq!(shared_run("a 248h2 b", "x248h24y"), None);
}
