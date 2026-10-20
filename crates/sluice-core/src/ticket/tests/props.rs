use std::net::SocketAddr;

use proptest::prelude::*;

use super::super::{Body, Format, Ticket};
use crate::Error;

fn socket_addr() -> impl Strategy<Value = SocketAddr> {
    prop_oneof![
        (any::<[u8; 4]>(), any::<u16>()).prop_map(|(ip, port)| SocketAddr::from((ip, port))),
        (any::<[u8; 16]>(), any::<u16>()).prop_map(|(ip, port)| SocketAddr::from((ip, port))),
    ]
}

fn body() -> impl Strategy<Value = Body> {
    prop_oneof![
        (any::<[u8; 32]>(), any::<u64>(), any::<bool>()).prop_map(|(root, size, hashes)| {
            Body::Static {
                root,
                size,
                format: if hashes {
                    Format::HashSequence
                } else {
                    Format::Raw
                },
            }
        }),
        (any::<[u8; 16]>(), any::<[u8; 32]>()).prop_map(|(stream_id, sender_key)| Body::Live {
            stream_id,
            sender_key
        }),
    ]
}

fn ticket() -> impl Strategy<Value = Ticket> {
    (
        any::<[u8; 32]>(),
        prop::collection::vec(socket_addr(), 0..8),
        prop::option::of("[a-z0-9:/.\u{e9}\u{65e5}-]{1,40}"),
        prop::option::of(any::<u64>()),
        body(),
    )
        .prop_map(|(id, addrs, relay, expiry, body)| {
            Ticket::new(id, addrs, relay, expiry, body).expect("a short ticket")
        })
}

proptest! {
    #[test]
    fn any_ticket_round_trips_and_reads_back_from_upper_case(ticket in ticket()) {
        let text = ticket.encode();
        prop_assert_eq!(Ticket::decode(&text).unwrap(), ticket.clone());
        prop_assert_eq!(Ticket::decode(&text.to_uppercase()).unwrap(), ticket);
    }

    #[test]
    fn a_ticket_with_one_character_changed_is_refused_or_has_that_one_spelling(
        ticket in ticket(),
        at in any::<prop::sample::Index>(),
        c in "[0-9a-zA-Z-]",
    ) {
        let mut chars: Vec<char> = ticket.encode().chars().collect();
        let index = at.index(chars.len());
        chars[index] = c.chars().next().unwrap();
        let mutated: String = chars.into_iter().collect();
        if let Ok(decoded) = Ticket::decode(&mutated) {
            prop_assert_eq!(decoded.encode(), mutated.to_ascii_lowercase());
        }
    }

    #[test]
    fn text_of_any_kind_is_refused_or_has_one_spelling(text in prop_oneof![
        any::<String>(),
        "sluice1[0-9a-hjkmnp-tv-z]{0,80}",
        "[0-9a-zA-Z-]{0,200}",
    ]) {
        match Ticket::decode(&text) {
            Ok(decoded) => prop_assert_eq!(decoded.encode(), text.to_ascii_lowercase()),
            Err(Error::Policy(message)) => {
                prop_assert!(message.starts_with("ticket "), "{message}");
                prop_assert!(message.len() < 200, "{message}");
            }
            Err(other) => prop_assert!(false, "{other:?}"),
        }
    }
}
