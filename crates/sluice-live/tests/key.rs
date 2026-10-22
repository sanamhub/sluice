//! The tests' secret key is the one the vectors were signed with. A wrong byte in it once went
//! unseen, since the tests signed and verified with the same wrong key and every one passed, while
//! the vectors carried RFC 8032's public key.

mod common;

use std::fs;
use std::path::Path;

/// The public key of RFC 8032 section 7.1, TEST 1, as the RFC prints it.
const RFC_8032_TEST_1_PUBLIC: &str =
    "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";

#[test]
fn the_tests_key_derives_the_public_key_of_rfc_8032_test_1() {
    let derived: String = common::public()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(derived, RFC_8032_TEST_1_PUBLIC);
}

#[test]
fn every_live_vector_file_carries_that_public_key() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/live");
    let wanted = format!("\"sender_key\": \"{RFC_8032_TEST_1_PUBLIC}\"");
    let mut files = 0;
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let text = fs::read_to_string(&path).unwrap();
            assert!(
                text.contains(&wanted),
                "{} has no case with it",
                path.display()
            );
            files += 1;
        }
    }
    assert_eq!(files, 6, "the live vector files");
}
