//! The receipt is a signature over 80 bytes and a label, so every refusal here is one a third
//! party's decoder must make too.

use ed25519_dalek::{Signer, SigningKey};

use super::{LEN, PREFIX, Receipt, commitment, refuse};
use crate::ticket::base32;

fn key() -> SigningKey {
    SigningKey::from_bytes(&[7; 32])
}

fn receipt() -> Receipt {
    Receipt::sign(&key(), commitment(&[9; 32]), 1024, 1_800_000_000_000)
}

/// The text form of 144 bytes, for the cases a signer cannot make: a key that must be refused, or a
/// signature under another label.
fn crafted(bytes: [u8; LEN]) -> String {
    format!("{PREFIX}{}", base32::encode(&bytes))
}

fn bytes() -> [u8; LEN] {
    let mut bytes = [0; LEN];
    bytes[..32].copy_from_slice(&commitment(&[9; 32]));
    bytes[32..40].copy_from_slice(&1024u64.to_le_bytes());
    bytes[40..72].copy_from_slice(key().verifying_key().as_bytes());
    bytes[72..80].copy_from_slice(&1_800_000_000_000u64.to_le_bytes());
    bytes
}

/// The bytes are what `SPEC.md` section 10 says, and the text is their base32 behind the prefix.
#[test]
fn a_signed_receipt_round_trips_with_its_fields() {
    let receipt = receipt();
    let text = receipt.encode();
    assert!(text.starts_with(PREFIX));
    assert_eq!(text.len(), PREFIX.len() + 231);
    let read = Receipt::decode(&text).unwrap();
    assert_eq!(read, receipt);
    assert_eq!(read.commit(), &commitment(&[9; 32]));
    assert!(read.is_of(&[9; 32]));
    assert_eq!(read.total_len(), 1024);
    assert_eq!(read.sender_key(), key().verifying_key().as_bytes());
    assert_eq!(read.utc_ms(), 1_800_000_000_000);
    read.verify().unwrap();
}

/// One receipt has one text, in either case, as a ticket does.
#[test]
fn the_text_is_read_in_either_case() {
    let receipt = receipt();
    assert_eq!(
        Receipt::decode(&receipt.encode().to_uppercase()).unwrap(),
        receipt
    );
}

/// The 144 bytes a record carries are the same receipt the text holds, and any other length is
/// refused before the bytes are read as fields.
#[test]
fn the_record_bytes_are_the_text_without_the_prefix() {
    let receipt = receipt();
    let bytes = base32::decode_bytes(&receipt.encode()[PREFIX.len()..]).unwrap();
    assert_eq!(Receipt::from_bytes(&bytes).unwrap(), receipt);
    for length in [0, 79, 143, 145] {
        let mut short = bytes.clone();
        short.resize(length, 0);
        assert!(Receipt::from_bytes(&short).is_err(), "{length}");
    }
}

/// A commitment names the content to someone who knows the root and to nobody else, and it is not
/// the root, so a receipt cannot be read as the body of a ticket.
#[test]
fn a_commitment_is_not_the_root() {
    let root = [9; 32];
    assert_ne!(commitment(&root), root);
    let receipt = receipt();
    assert!(receipt.is_of(&root));
    assert!(!receipt.is_of(&[10; 32]));
    // A receipt whose commit is the root itself, which is what an implementation that skipped the
    // commitment would write, is not about the content with that root.
    let naive = Receipt::sign(&key(), root, 1024, 1_800_000_000_000);
    assert!(!naive.is_of(&root));
    naive.verify().unwrap();
}

/// A text file ends with a newline, so one is allowed; a second is not, and neither is anything
/// else after the body. A `\r` is only an ending when a `\n` follows it: a lone one is a character
/// outside the alphabet, and is refused as one.
#[test]
fn one_line_ending_is_allowed_and_nothing_else() {
    let text = receipt().encode();
    assert!(Receipt::decode(&format!("{text}\n")).is_ok());
    assert!(Receipt::decode(&format!("{text}\r\n")).is_ok());
    assert!(Receipt::decode(&format!("{text}\n\n")).is_err());
    assert!(Receipt::decode(&format!("{text} ")).is_err());
    assert!(Receipt::decode(&format!("{text}0")).is_err());
    assert!(Receipt::decode(&format!(" {text}")).is_err());
    let lone = Receipt::decode(&format!("{text}\r")).unwrap_err();
    assert!(lone.to_string().contains("no `\\n` follows"), "{lone}");
    // A `\r` in the middle is still just a character the alphabet does not hold.
    assert!(Receipt::decode(&format!("{text}\r\n\r")).is_err());
}

/// Each of the 80 signed bytes decides the signature, so a changed length is a refused receipt.
#[test]
fn a_changed_byte_is_refused() {
    let mut bytes = bytes();
    bytes[32..40].copy_from_slice(&1025u64.to_le_bytes());
    let changed = Receipt::decode(&crafted(bytes)).unwrap();
    let error = changed.verify().unwrap_err();
    assert_eq!(error.kind(), crate::ErrorKind::Verify);
    assert!(error.to_string().contains("does not verify"), "{error}");
}

/// The label is what keeps a live record's signature from being good for a receipt.
#[test]
fn a_signature_under_the_live_label_is_refused() {
    let signed = {
        let receipt = receipt();
        let mut signed = [0; 80];
        signed[..32].copy_from_slice(receipt.commit());
        signed[32..40].copy_from_slice(&receipt.total_len().to_le_bytes());
        signed[40..72].copy_from_slice(receipt.sender_key());
        signed[72..80].copy_from_slice(&receipt.utc_ms().to_le_bytes());
        signed
    };
    let mut message = Vec::new();
    message.extend_from_slice(b"sluice/live/entry/1");
    message.push(0);
    message.extend_from_slice(&[3; 16]);
    message.extend_from_slice(&signed);
    let mut bytes = bytes();
    bytes[80..].copy_from_slice(&key().sign(&message).to_bytes());
    let error = Receipt::decode(&crafted(bytes))
        .unwrap()
        .verify()
        .unwrap_err();
    assert!(error.to_string().contains("does not verify"), "{error}");
}

/// Rule 8 of section 4: a key that is not in its one encoding is refused, whatever the signature
/// says. `0xff` in every byte is a `y` at or above the field's prime, which a strict encoder never
/// writes.
#[test]
fn a_key_that_is_not_canonical_is_refused() {
    let mut bytes = bytes();
    bytes[40..72].copy_from_slice(&[0xff; 32]);
    let error = Receipt::decode(&crafted(bytes))
        .unwrap()
        .verify()
        .unwrap_err();
    assert!(error.to_string().contains("canonical"), "{error}");
}

/// A key of small order verifies nothing strictly, so a receipt that names one is refused before
/// its signature is looked at.
#[test]
fn a_small_order_key_is_refused() {
    let mut identity = [0; 32];
    identity[0] = 1;
    let mut bytes = bytes();
    bytes[40..72].copy_from_slice(&identity);
    let error = Receipt::decode(&crafted(bytes))
        .unwrap()
        .verify()
        .unwrap_err();
    assert!(error.to_string().contains("large order"), "{error}");
}

/// A body that is not 144 bytes is refused with its length and never its bytes, and one so long
/// that its text is longer than a receipt's is refused before it is decoded.
#[test]
fn a_short_or_long_body_is_refused() {
    for length in [0, 79, 143, 145] {
        let text = crafted([0; LEN]);
        let bytes = base32::decode_bytes(&text[PREFIX.len()..]).unwrap();
        let mut short = bytes.clone();
        short.resize(length, 0);
        let text = format!("{PREFIX}{}", base32::encode(&short));
        let error = Receipt::decode(&text).unwrap_err();
        assert!(
            error.to_string().contains("receipt is 144"),
            "{length}: {error}"
        );
    }
    let bytes = base32::decode_bytes(&crafted([0; LEN])[PREFIX.len()..]).unwrap();
    let mut long = bytes.to_vec();
    long.resize(200, 0);
    let text = format!("{PREFIX}{}", base32::encode(&long));
    let error = Receipt::decode(&text).unwrap_err();
    assert!(error.to_string().contains("at most"), "{error}");
}

/// Text that is not a receipt at all is refused by prefix, and text over the length a receipt can
/// be is refused before it is decoded at all.
#[test]
fn a_prefix_that_is_not_its_own_is_refused() {
    let text = receipt().encode();
    let error = Receipt::decode(&text.replacen(PREFIX, "sluice1", 1)).unwrap_err();
    assert!(error.to_string().contains("prefix"), "{error}");
    let error = Receipt::decode(&"a".repeat(300)).unwrap_err();
    assert!(error.to_string().contains("at most"), "{error}");
}

/// A body of the right length with a character outside the alphabet names the byte, never the
/// character.
#[test]
fn a_character_outside_the_alphabet_is_refused_by_place() {
    let mut text = receipt().encode();
    text.replace_range(PREFIX.len() + 3..PREFIX.len() + 4, "-");
    let error = Receipt::decode(&text).unwrap_err();
    assert!(
        error.to_string().contains("outside the alphabet at byte 3"),
        "{error}"
    );
}

/// The refusal is built from a reason and never quotes the receipt.
#[test]
fn a_refusal_never_quotes_the_receipt() {
    let error = refuse("is not a receipt");
    assert!(error.to_string().starts_with("policy denied: receipt "));
}
