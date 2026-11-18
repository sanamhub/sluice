use crate::code::Code;
use crate::crypto::{CryptoError, Exchange, Keys, NONCE_LEN, transcript};
use crate::frame::Role;

fn code(text: &str) -> Code {
    Code::parse(text).unwrap()
}

#[test]
fn both_sides_derive_the_same_tags_and_the_peer_tag_verifies() {
    let c = code("7-orbit-velvet");
    let sender = Exchange::new(Role::Sender, &c);
    let receiver = Exchange::new(Role::Receiver, &c);
    let (a, b) = (*sender.outbound(), *receiver.outbound());
    let sender_keys = sender.finish(&b).unwrap();
    let receiver_keys = receiver.finish(&a).unwrap();
    assert_eq!(
        sender_keys.confirmation(Role::Sender),
        receiver_keys.confirmation(Role::Sender)
    );
    assert_eq!(
        sender_keys.confirmation(Role::Receiver),
        receiver_keys.confirmation(Role::Receiver)
    );
    assert!(
        receiver_keys
            .verify(Role::Sender, &sender_keys.confirmation(Role::Sender))
            .is_ok()
    );
    assert!(
        sender_keys
            .verify(Role::Receiver, &receiver_keys.confirmation(Role::Receiver))
            .is_ok()
    );
}

#[test]
fn a_different_word_pair_fails_the_confirmation() {
    let sender = Exchange::new(Role::Sender, &code("7-orbit-velvet"));
    let receiver = Exchange::new(Role::Receiver, &code("7-orbit-vendor"));
    let (a, b) = (*sender.outbound(), *receiver.outbound());
    let sender_keys = sender.finish(&b).unwrap();
    let receiver_keys = receiver.finish(&a).unwrap();
    assert_eq!(
        sender_keys.verify(Role::Receiver, &receiver_keys.confirmation(Role::Receiver)),
        Err(CryptoError::WrongTag)
    );
}

#[test]
fn the_ticket_seals_and_opens_with_the_transcript_bound() {
    let c = code("7-orbit-velvet");
    let sender = Exchange::new(Role::Sender, &c);
    let receiver = Exchange::new(Role::Receiver, &c);
    let (a, b) = (*sender.outbound(), *receiver.outbound());
    let sender_keys = sender.finish(&b).unwrap();
    let receiver_keys = receiver.finish(&a).unwrap();
    let sealed = sender_keys.seal("sluice1abcdefghjkmnpqrstvwxyz").unwrap();
    assert_eq!(
        receiver_keys.open(&sealed).unwrap().as_str(),
        "sluice1abcdefghjkmnpqrstvwxyz"
    );
}

#[test]
fn a_seal_from_another_transcript_does_not_open() {
    let first = Keys::from_shared(&[1; 32], b"one transcript").unwrap();
    let second = Keys::from_shared(&[1; 32], b"another transcript").unwrap();
    let sealed = first
        .seal_with_nonce(&[0; NONCE_LEN], "sluice1abc")
        .unwrap();
    assert!(matches!(second.open(&sealed), Err(CryptoError::Aead)));
}

#[test]
fn a_changed_tag_does_not_open() {
    let keys = Keys::from_shared(&[2; 32], b"transcript").unwrap();
    let mut sealed = keys.seal_with_nonce(&[3; NONCE_LEN], "sluice1abc").unwrap();
    let last = sealed.len() - 1;
    sealed[last] ^= 1;
    assert!(matches!(keys.open(&sealed), Err(CryptoError::Aead)));
}

#[test]
fn the_transcript_is_the_specified_shape() {
    let got = transcript(7, b"AAA", b"BBB");
    let want = b"sluice short code v1\x00\x00\x00\x07\x00\x00\x00\x03AAA\x00\x00\x00\x03BBB";
    assert_eq!(got, want);
}

#[test]
fn debug_never_shows_a_key_or_a_tag() {
    let keys = Keys::from_shared(&[2; 32], b"transcript").unwrap();
    let shown = format!("{keys:?}");
    for byte in keys.confirmation(Role::Sender) {
        assert!(!shown.contains(&format!("{byte:02x}")), "{shown}");
    }
    assert!(!shown.contains("020202"), "{shown}");
}

#[test]
fn a_short_peer_message_is_refused() {
    let sender = Exchange::new(Role::Sender, &code("7-orbit-velvet"));
    assert!(matches!(sender.finish(&[0; 32]), Err(CryptoError::Spake)));
}
