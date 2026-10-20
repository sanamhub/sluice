use std::net::SocketAddr;

use super::super::{Body, Format, Ticket, base32, wire};
use super::common::*;

#[test]
fn the_bytes_follow_the_table_in_the_spec() {
    let ticket = Ticket::new(
        ID,
        vec![addr("192.0.2.1:50000"), addr("[2001:db8::1]:443")],
        Some("r".into()),
        Some(300),
        Body::Static {
            root: ROOT,
            size: 300,
            format: Format::HashSequence,
        },
    )
    .unwrap();
    let mut expected = vec![0x01, 0x00];
    expected.extend(ID);
    expected.push(2);
    expected.extend([0x00, 192, 0, 2, 1, 0xd0, 0x86, 0x03]);
    expected.push(0x01);
    expected.extend([
        0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0xbb, 0x03,
    ]);
    expected.extend([0x01, 0x01, b'r']);
    expected.extend([0x01, 0xac, 0x02]);
    expected.extend(ROOT);
    expected.extend([0xac, 0x02, 0x01]);
    assert_eq!(wire::write(&ticket), expected);
    assert_eq!(Ticket::decode(&text(&expected)).unwrap(), ticket);
}

#[test]
fn a_live_ticket_has_no_tag_for_its_body() {
    let ticket = Ticket::new(ID, vec![], None, None, live_body()).unwrap();
    let bytes = wire::write(&ticket);
    assert_eq!(&bytes[..2], [1, 1]);
    assert_eq!(&bytes[34..37], [0, 0, 0]);
    assert_eq!(&bytes[37..53], [0x33; 16]);
    assert_eq!(&bytes[53..], [0x44; 32]);
}

#[test]
fn the_sizes_in_the_spec_are_the_measured_ones() {
    let v4 = vec![addr("192.0.2.1:50000"), addr("198.51.100.7:50000")];
    let v6 = vec![addr("[2001:db8::1]:50000"), addr("[2001:db8::2]:50000")];
    let size = |addrs: Vec<SocketAddr>, body: Body| {
        let ticket = Ticket::new(ID, addrs, None, None, body).unwrap();
        (wire::write(&ticket).len(), ticket.encode().len())
    };
    assert_eq!(size(v4.clone(), static_body(100 << 20)), (90, 151));
    assert_eq!(size(v6, static_body(100 << 20)), (114, 190));
    assert_eq!(size(v4, live_body()), (101, 169));
}

#[test]
fn the_base32_alphabet_and_bit_order_are_crockfords() {
    assert_eq!(base32::encode(b"foobar"), "csqpyrk1e8");
    assert_eq!(base32::decode("csqpyrk1e8").unwrap(), b"foobar");
    assert_eq!(base32::decode("CSQPYRK1E8").unwrap(), b"foobar");
    assert_eq!(base32::decode("CsQpYrK1e8").unwrap(), b"foobar");
    assert_eq!(base32::encode(&[]), "");
    assert_eq!(base32::encode(&[0xff]), "zw");
}

#[test]
fn base32_takes_only_lengths_whole_bytes_can_give() {
    for n in 0..24 {
        let result = base32::decode(&"0".repeat(n));
        assert_eq!(
            result.is_ok(),
            !matches!(n % 8, 1 | 3 | 6),
            "{n} characters"
        );
    }
}

#[test]
fn base32_rejects_unused_bits_that_are_not_zero() {
    // "00" is the byte 0 and 2 spare bits. "03" has the last of them set.
    assert_eq!(base32::decode("00").unwrap(), [0]);
    assert!(base32::decode("01").is_err());
    assert!(base32::decode("0z").is_err());
    assert_eq!(base32::decode("zw").unwrap(), [0xff]);
    assert!(base32::decode("zx").is_err());
}

#[test]
fn base32_rejects_every_character_outside_the_alphabet() {
    let mut bad: Vec<char> = "ilouILOU-=_ \t\n+/.é\u{0}日".chars().collect();
    bad.extend(
        ('!'..='/')
            .chain(':'..='@')
            .chain('['..='`')
            .chain('{'..='~'),
    );
    for c in bad {
        let candidate = format!("0{c}");
        assert!(base32::decode(&candidate).is_err(), "{c:?}");
    }
}

#[test]
fn an_address_pair_round_trips_in_both_families() {
    for addrs in [
        vec![],
        vec![addr("0.0.0.0:0")],
        vec![addr("[::]:65535"), addr("255.255.255.255:1")],
    ] {
        let ticket = Ticket::new(
            ID,
            addrs,
            Some("https://relay.example".into()),
            Some(u64::MAX),
            static_body(u64::MAX),
        )
        .unwrap();
        assert_eq!(Ticket::decode(&ticket.encode()).unwrap(), ticket);
    }
}

#[test]
fn text_and_prefix_are_read_in_any_case_and_written_in_lower_case() {
    let ticket = Ticket::new(ID, vec![addr("192.0.2.1:50000")], None, None, live_body()).unwrap();
    let lower = ticket.encode();
    assert_eq!(lower, lower.to_lowercase());
    let mixed: String = lower
        .chars()
        .enumerate()
        .map(|(i, c)| {
            if i % 2 == 0 {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect();
    for spelling in [
        lower.to_uppercase(),
        mixed,
        format!("Sluice1{}", &lower[7..]),
    ] {
        assert_eq!(Ticket::decode(&spelling).unwrap(), ticket, "{spelling}");
    }
}
