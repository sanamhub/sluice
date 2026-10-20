use super::super::Ticket;
use super::common::*;

#[test]
fn a_ticket_with_the_wrong_start_is_refused() {
    let good = Ticket::new(ID, vec![], None, None, live_body())
        .unwrap()
        .encode();
    let body = &good[7..];
    for bad in [
        "",
        "sluice",
        "sluice2",
        "ticket1",
        "\u{e9}luice1",
        "sluic\u{e9}1",
        "sluice\u{e9}",
    ] {
        let message = refusal(&format!("{bad}{body}"));
        assert!(
            message.contains("start with sluice1") || message.contains("outside the alphabet"),
            "{message}"
        );
    }
    assert!(refusal("").contains("does not start with sluice1"));
    assert!(refusal("sluice1").contains("is empty"));
    assert!(refusal(body).contains("does not start with sluice1"));
    // The prefix has to be at the start: a space, a newline or a second prefix are refused.
    for bad in [
        format!(" {good}"),
        format!("{good}\n"),
        format!("{good} "),
        format!("sluice1{good}"),
    ] {
        refusal(&bad);
    }
}

#[test]
fn every_character_outside_the_alphabet_is_refused_in_a_whole_ticket() {
    let good = Ticket::new(ID, vec![addr("192.0.2.1:50000")], None, None, live_body())
        .unwrap()
        .encode();
    for c in "ilouILOU-=_ \t\n.é日".chars() {
        let mut bad = good.clone();
        bad.insert(20, c);
        assert!(
            refusal(&bad).contains("outside the alphabet at byte"),
            "{c:?}"
        );
        let mut replaced = good.clone();
        replaced.replace_range(20..21, &c.to_string());
        assert!(
            refusal(&replaced).contains("outside the alphabet at byte"),
            "{c:?}"
        );
    }
}

#[test]
fn a_body_length_whole_bytes_cannot_give_is_refused() {
    let good = Ticket::new(ID, vec![], None, None, live_body())
        .unwrap()
        .encode();
    for extra in ["0", "000", "000000"] {
        assert!(
            refusal(&format!("{good}{extra}")).contains("whole bytes"),
            "{extra}"
        );
    }
}

#[test]
fn unused_bits_that_are_not_zero_are_refused() {
    // 72 bytes are 576 bits, which is 115 full characters and one with 1 data bit and 4 spare ones.
    let good = text(&Parts::valid().bytes());
    let last = good.chars().last().unwrap();
    let alphabet = "0123456789abcdefghjkmnpqrstvwxyz";
    let index = alphabet.find(last).unwrap();
    for set in [1, 2, 4, 8] {
        let mut bad = good.clone();
        bad.pop();
        bad.push(alphabet.as_bytes()[index | set] as char);
        assert!(refusal(&bad).contains("non-zero unused bits"), "{set}");
    }
}

#[test]
fn an_unknown_version_kind_or_format_is_refused() {
    for version in [0u8, 2, 3, 255] {
        let parts = Parts {
            version,
            ..Parts::valid()
        };
        assert!(
            refusal_of(&parts.bytes()).contains("is not supported"),
            "{version}"
        );
    }
    for kind in [2u8, 3, 255] {
        let parts = Parts {
            kind,
            ..Parts::valid()
        };
        assert!(
            refusal_of(&parts.bytes()).contains("kind is not 0 or 1"),
            "{kind}"
        );
    }
    for format in [2u8, 3, 255] {
        let mut parts = Parts::valid();
        *parts.body.last_mut().unwrap() = format;
        assert!(
            refusal_of(&parts.bytes()).contains("format is not 0 or 1"),
            "{format}"
        );
    }
    assert!(Ticket::decode(&text(&Parts::valid().bytes())).is_ok());
}

#[test]
fn bytes_after_the_body_and_bytes_missing_from_it_are_refused() {
    let valid = Parts::valid().bytes();
    for extra in [vec![0u8], vec![0, 0, 0], vec![1]] {
        let mut bytes = valid.clone();
        bytes.extend(extra);
        assert!(refusal_of(&bytes).contains("bytes after the body"));
    }
    for keep in [0, 1, 2, 3, 20, 34, 35, 36, 37, valid.len() - 1] {
        let message = refusal_of(&valid[..keep]);
        assert!(
            message.contains("is empty") || message.contains("cut short"),
            "{keep}: {message}"
        );
    }
}

#[test]
fn an_empty_relay_url_is_refused_and_a_missing_one_is_not() {
    let parts = Parts {
        relay: vec![1, 0],
        ..Parts::valid()
    };
    assert!(refusal_of(&parts.bytes()).contains("empty relay_url"));
    let parts = Parts {
        relay: vec![1, 1, b'r'],
        ..Parts::valid()
    };
    assert_eq!(
        Ticket::decode(&text(&parts.bytes())).unwrap().relay_url(),
        Some("r")
    );
    assert_eq!(
        Ticket::decode(&text(&Parts::valid().bytes()))
            .unwrap()
            .relay_url(),
        None
    );
}

#[test]
fn tags_and_strings_that_postcard_would_read_wrongly_are_refused() {
    let one_addr = |tag: u8| vec![1, tag, 192, 0, 2, 1, 80];
    let cases = [
        (
            "option tag 2 for the relay",
            Parts {
                relay: vec![2],
                ..Parts::valid()
            },
        ),
        (
            "option tag 2 for the expiry",
            Parts {
                expiry: vec![2],
                ..Parts::valid()
            },
        ),
        (
            "address tag 2",
            Parts {
                addrs: one_addr(2),
                ..Parts::valid()
            },
        ),
        (
            "relay that is not UTF-8",
            Parts {
                relay: vec![1, 2, 0xc3, 0x28],
                ..Parts::valid()
            },
        ),
        (
            "relay longer than the bytes left",
            Parts {
                relay: vec![1, 200, 1, b'a'],
                ..Parts::valid()
            },
        ),
    ];
    for (what, parts) in cases {
        assert!(
            refusal_of(&parts.bytes()).contains("cut short or malformed"),
            "{what}"
        );
    }
    assert!(
        Ticket::decode(&text(
            &Parts {
                addrs: one_addr(0),
                ..Parts::valid()
            }
            .bytes()
        ))
        .is_ok()
    );
}

#[test]
fn an_address_count_larger_than_the_bytes_is_refused_without_allocating_for_it() {
    for count in [varint(1 << 40), varint(u64::MAX)] {
        let mut addrs = count;
        addrs.extend([0, 192, 0, 2, 1, 80]);
        let parts = Parts {
            addrs,
            ..Parts::valid()
        };
        assert!(refusal_of(&parts.bytes()).contains("more addresses than it has bytes for"));
    }
}

#[test]
fn a_varint_longer_than_it_needs_to_be_is_refused() {
    let cases = [
        (
            "expiry 0 in two bytes",
            Parts {
                expiry: vec![1, 0x80, 0x00],
                ..Parts::valid()
            },
        ),
        (
            "expiry 1 in three bytes",
            Parts {
                expiry: vec![1, 0x81, 0x80, 0x00],
                ..Parts::valid()
            },
        ),
        (
            "address count 0 in two bytes",
            Parts {
                addrs: vec![0x80, 0x00],
                ..Parts::valid()
            },
        ),
        (
            "port 80 in two bytes",
            Parts {
                addrs: vec![1, 0, 192, 0, 2, 1, 0xd0, 0x00],
                ..Parts::valid()
            },
        ),
        (
            "relay length 1 in two bytes",
            Parts {
                relay: vec![1, 0x81, 0x00, b'r'],
                ..Parts::valid()
            },
        ),
    ];
    for (what, parts) in cases {
        assert!(
            refusal_of(&parts.bytes()).contains("shortest byte form"),
            "{what}"
        );
    }
    let mut size = ROOT.to_vec();
    size.extend([0xac, 0x82, 0x00, 0]);
    assert!(
        refusal_of(
            &Parts {
                body: size,
                ..Parts::valid()
            }
            .bytes()
        )
        .contains("shortest byte form")
    );
    // The shortest forms of the same values are fine.
    let parts = Parts {
        expiry: vec![1, 0x00],
        addrs: vec![1, 0, 192, 0, 2, 1, 0x50],
        ..Parts::valid()
    };
    assert!(Ticket::decode(&text(&parts.bytes())).is_ok());
}

#[test]
fn a_varint_that_does_not_fit_is_refused() {
    let mut too_long = vec![1];
    too_long.extend([0xff; 10]);
    too_long.push(0x7f);
    let parts = Parts {
        expiry: too_long,
        ..Parts::valid()
    };
    assert!(refusal_of(&parts.bytes()).contains("cut short or malformed"));
    let parts = Parts {
        addrs: vec![1, 0, 1, 2, 3, 4, 0xff, 0xff, 0x7f],
        ..Parts::valid()
    };
    assert!(refusal_of(&parts.bytes()).contains("cut short or malformed"));
}

#[test]
fn text_over_4096_bytes_is_refused_before_it_is_decoded() {
    let good = Ticket::new(ID, vec![], None, None, live_body())
        .unwrap()
        .encode();
    // Valid base32 all the way, so only the length can be what refuses it.
    let long = format!("{good}{}", "0".repeat(4097 - good.len()));
    assert_eq!(long.len(), 4097);
    assert!(refusal(&long).contains("longer than 4096 bytes"));
    assert!(refusal(&format!("{long}\u{e9}")).contains("longer than 4096 bytes"));
    assert!(refusal(&"x".repeat(1 << 20)).contains("longer than 4096 bytes"));
}
