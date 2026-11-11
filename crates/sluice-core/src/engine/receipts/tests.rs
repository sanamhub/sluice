//! The rules of section 10, as decisions: what a sender answers, and what a receiver keeps.

use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;

use super::{CLOCK_WINDOW_MS, Receipts, accept};
use crate::engine::conn::Peer;
use crate::engine::policy::{Asked, Places};
use crate::receipt::{Receipt, commitment};

const NOW: u64 = 1_800_000_000_000;
const ROOT: [u8; 32] = [9; 32];

/// The seed the sender signs with, and the endpoint id it puts in a ticket.
fn sender_seed() -> [u8; 32] {
    [7; 32]
}

fn signer() -> SigningKey {
    SigningKey::from_bytes(&sender_seed())
}

/// The sender's public key, which is the endpoint id a receiver checks a receipt against.
fn sender_key() -> [u8; 32] {
    signer().verifying_key().to_bytes()
}

/// A receipt a sender makes for `root` at `at`, with `len` bytes.
fn signed(root: [u8; 32], len: u64, at: u64) -> Receipt {
    Receipt::sign(&signer(), commitment(&root), len, at)
}

/// Bytes of a record for a receipt, in the layout section 10 gives.
fn record(receipt: &Receipt) -> Vec<u8> {
    receipt.to_bytes().to_vec()
}

fn peer(byte: u8) -> Peer {
    Peer {
        endpoint: [byte; 32],
        relayed: false,
    }
}

/// A sender that offers a receipt for [`ROOT`] and `1024` bytes, with a policy that let `peer(1)`
/// ask for it.
fn sender(asked_for_root: bool) -> (Arc<Receipts>, Peer) {
    let asked = Arc::new(Mutex::new(Asked::default()));
    let places = Arc::new(Mutex::new(Places::default()));
    {
        let mut places = places.lock().unwrap();
        places.set_limit(None);
    }
    if asked_for_root {
        asked
            .lock()
            .unwrap()
            .remember(1, peer(1), iroh_blobs::Hash::from(ROOT));
    }
    let receipts = Receipts::new(asked, places);
    receipts.offer(ROOT, 1024, sender_seed(), None);
    (receipts, peer(1))
}

/// The receipt a sender writes is about the root it serves, carries its length, is signed under its
/// identity key, and names the time of the answer.
#[test]
fn an_admitted_asker_gets_a_receipt_for_what_is_served() {
    let (receipts, asker) = sender(true);
    let receipt = receipts.answer(asker.endpoint, ROOT, NOW).unwrap();
    receipt.verify().unwrap();
    assert_eq!(receipt.sender_key(), &sender_key());
    assert_eq!(receipt.total_len(), 1024);
    assert!(receipt.is_of(&ROOT));
    assert_eq!(receipt.utc_ms(), NOW);
    assert_eq!(receipts.answered(), 1);
}

/// The policy decides: an endpoint that never asked for the root is answered with nothing, and so
/// is an endpoint that asked for another root.
#[test]
fn a_peer_the_policy_did_not_admit_gets_nothing() {
    let (receipts, asker) = sender(false);
    assert!(receipts.answer(asker.endpoint, ROOT, NOW).is_none());
    assert_eq!(receipts.answered(), 0);

    let (receipts, asker) = sender(true);
    assert!(receipts.answer(asker.endpoint, [10; 32], NOW).is_none());
    assert!(receipts.answer([11; 32], ROOT, NOW).is_none());
    assert_eq!(receipts.answered(), 0);
}

/// A limited send answers only a peer that holds a place, even when it asked for the root before
/// the places were set.
#[test]
fn a_limited_send_answers_only_a_holder() {
    let asked = Arc::new(Mutex::new(Asked::default()));
    asked
        .lock()
        .unwrap()
        .remember(1, peer(1), iroh_blobs::Hash::from(ROOT));
    let places = Arc::new(Mutex::new(Places::default()));
    {
        // One place, taken by another endpoint.
        let mut locked = places.lock().unwrap();
        locked.set_limit(Some(1));
        locked.admit([12; 32], 1);
    }
    let receipts = Receipts::new(asked, places);
    receipts.offer(ROOT, 1024, sender_seed(), None);
    assert!(receipts.answer([1; 32], ROOT, NOW).is_none());
}

/// A sender that has cleared the bit, or whose ticket has expired, writes nothing.
#[test]
fn a_disabled_or_expired_sender_gets_nothing() {
    let (receipts, asker) = sender(true);
    receipts.disable();
    assert!(receipts.answer(asker.endpoint, ROOT, NOW).is_none());

    let (receipts, asker) = sender(true);
    receipts.offer(ROOT, 1024, sender_seed(), Some(NOW));
    assert!(receipts.answer(asker.endpoint, ROOT, NOW).is_none());
    assert!(
        receipts
            .answer(asker.endpoint, ROOT, NOW.saturating_sub(1))
            .is_some()
    );
}

/// Every rule of section 10 that a received record must pass, one at a time.
#[test]
fn a_record_is_kept_only_when_every_rule_holds() {
    let receipt = signed(ROOT, 1024, NOW);
    assert!(accept(&record(&receipt), &ROOT, 1024, &sender_key(), NOW).is_some());

    // Rule 1: the length of the record.
    let mut short = record(&receipt);
    short.pop();
    assert!(accept(&short, &ROOT, 1024, &sender_key(), NOW).is_none());

    // Rule 2: the key is not the endpoint the content came from.
    let other = Receipt::sign(
        &SigningKey::from_bytes(&[8; 32]),
        commitment(&ROOT),
        1024,
        NOW,
    );
    assert!(accept(&record(&other), &ROOT, 1024, &sender_key(), NOW).is_none());

    // Rule 3: another root, and another length.
    let another_root = signed([10; 32], 1024, NOW);
    assert!(accept(&record(&another_root), &ROOT, 1024, &sender_key(), NOW).is_none());
    let another_len = signed(ROOT, 1025, NOW);
    assert!(accept(&record(&another_len), &ROOT, 1024, &sender_key(), NOW).is_none());

    // Rule 4: the clock window, with its boundary.
    let late = signed(ROOT, 1024, NOW + CLOCK_WINDOW_MS);
    assert!(accept(&record(&late), &ROOT, 1024, &sender_key(), NOW).is_some());
    let too_late = signed(ROOT, 1024, NOW + CLOCK_WINDOW_MS + 1);
    assert!(accept(&record(&too_late), &ROOT, 1024, &sender_key(), NOW).is_none());
    let early = signed(ROOT, 1024, NOW.saturating_sub(CLOCK_WINDOW_MS + 1));
    assert!(accept(&record(&early), &ROOT, 1024, &sender_key(), NOW).is_none());

    // Rule 5: the signature.
    let mut changed = record(&receipt);
    let at = changed.len() - 1;
    changed[at] ^= 1;
    assert!(accept(&changed, &ROOT, 1024, &sender_key(), NOW).is_none());
}

/// A receipt whose commit is the root itself is not about that content, so it is an absence.
#[test]
fn a_commit_that_is_the_root_is_an_absence() {
    let naive = Receipt::sign(&signer(), ROOT, 1024, NOW);
    naive.verify().unwrap();
    assert!(accept(&record(&naive), &ROOT, 1024, &sender_key(), NOW).is_none());
}
