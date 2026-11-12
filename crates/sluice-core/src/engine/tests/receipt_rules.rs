//! The rules a receiver applies to a record, one case per rule, each through a real transfer: the
//! record that breaks it is absent and the transfer that carried it finished.

use std::time::Duration;

use super::receipts::{BODY, with_receipt};
use super::*;
use crate::receipt::Receipt;

/// A record that breaks one receiver rule is absent, and the transfer that carried it finished:
/// one case per rule the record itself decides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_record_that_breaks_a_rule_is_absent_and_the_transfer_finishes() {
    let now = now_ms();
    type Build = Box<dyn Fn([u8; 32], u64, [u8; 32]) -> Vec<u8>>;
    let cases: Vec<(&str, Build)> = vec![
        // Rule 1: the record's length is not 144.
        (
            "length",
            Box::new(move |_root, _size, _key| {
                let mut bytes = crate::engine::hello::varint_bytes(143);
                bytes.extend_from_slice(&[0; 143]);
                bytes
            }),
        ),
        // Rule 2: the key is not the endpoint the content came from.
        (
            "sender_key",
            Box::new(move |root, size, _key| {
                let other = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&[8; 32]),
                    crate::receipt::commitment(&root),
                    size,
                    now,
                );
                record(&other)
            }),
        ),
        // Rule 3: the commitment is another root's.
        (
            "root",
            Box::new(move |_root, size, key| {
                let other = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&[10; 32]),
                    size,
                    now,
                );
                record(&other)
            }),
        ),
        // Rule 4: the time is outside the clock window.
        (
            "time",
            Box::new(move |root, size, key| {
                let late = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&root),
                    size,
                    now + 300_001,
                );
                record(&late)
            }),
        ),
        // Rule 5: the signature does not hold.
        (
            "signature",
            Box::new(move |root, size, key| {
                let good = Receipt::sign(
                    &ed25519_dalek::SigningKey::from_bytes(&key),
                    crate::receipt::commitment(&root),
                    size,
                    now,
                );
                let mut bytes = record(&good);
                let at = bytes.len() - 1;
                bytes[at] ^= 1;
                bytes
            }),
        ),
    ];
    for (name, build) in cases {
        let dir = tempfile::tempdir().unwrap();
        let (outgoing, text) = prepared(&dir, BODY).await;
        let ticket = Ticket::decode(&text).unwrap();
        let Body::Static { root, size, .. } = ticket.body() else {
            panic!("a file ticket is static");
        };
        outgoing
            .provider
            .receipts()
            .force_record(build(*root, *size, *ticket.endpoint_id()));
        let serve = tokio::spawn(async move { outgoing.serve(Keep::One, &no_events()).await });
        let out = dir.path().join("received");
        let receipt_path = dir.path().join("receipt.txt");
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let received = super::receive(&text, &with_receipt(&out, &receipt_path), &tx)
            .await
            .unwrap_or_else(|e| panic!("{name}: the transfer failed: {e}"));
        assert_eq!(received.bytes, BODY.len() as u64, "{name}");
        let end = tokio::time::timeout(Duration::from_secs(15), serve)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(end, super::StopEnd::Downloaded, "{name}");
        assert!(!receipt_path.exists(), "{name}: a receipt was written");
    }
}

fn record(receipt: &Receipt) -> Vec<u8> {
    let mut bytes = crate::engine::hello::varint_bytes(144);
    bytes.extend_from_slice(&receipt.to_bytes());
    bytes
}
