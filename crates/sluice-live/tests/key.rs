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

/// The cases of `open.json` that are about the ticket's key being wrong, so their key is not the
/// RFC's. They are named here, and no other case may differ: a case with another key by accident
/// would be signed by a key no one has, and pass or fail for that.
const OTHER_KEYS: [(&str, &str); 3] = [
    (
        "an open that names another sender than the ticket's",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
    ),
    (
        "a ticket key of small order, with a signature that holds for any message",
        "0100000000000000000000000000000000000000000000000000000000000000",
    ),
    (
        "a ticket key out of its one encoding: y = 2^255 - 1, which names y = 18",
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ),
];

#[test]
fn every_case_of_every_live_vector_file_carries_that_public_key() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors/live");
    let (mut files, mut cases, mut others) = (0, 0, 0);
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let file: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let file_cases = file["cases"].as_array().expect("a cases array");
        assert!(!file_cases.is_empty(), "{} has no cases", path.display());
        for case in file_cases {
            let (name, key) = (case["name"].as_str().unwrap(), case["sender_key"].as_str());
            let wanted = match OTHER_KEYS.iter().find(|(other, _)| *other == name) {
                Some((_, other)) => {
                    others += 1;
                    *other
                }
                None => RFC_8032_TEST_1_PUBLIC,
            };
            assert_eq!(key, Some(wanted), "{}: {name}", path.display());
            cases += 1;
        }
        files += 1;
    }
    assert_eq!(files, 6, "the live vector files");
    assert_eq!(others, OTHER_KEYS.len(), "every named exception is a case");
    assert!(cases > others, "{cases} cases");
}
