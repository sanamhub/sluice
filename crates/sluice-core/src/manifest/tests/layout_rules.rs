//! Rule 16: the packed entries lie end to end and fill the packs. Each pack layout rule has a case
//! that keeps it and a case that breaks it, and the decoder and `Manifest::new` agree on both.

use super::common::*;
use crate::manifest::{Entry, Kind, Loc, Manifest, PackRef, wire};

fn at(name: &str, pack_index: u32, offset: u64, len: u64) -> Entry {
    let loc = Loc::Packed {
        pack_index,
        offset,
        len,
    };
    typed(name, Kind::File, loc, len)
}

/// The rule the manifest breaks, or `None`. It asks the decoder and `new`, which must agree.
fn verdict(entries: Vec<Entry>, sizes: &[u64]) -> Option<(u8, Option<u8>)> {
    let packs: Vec<PackRef> = sizes
        .iter()
        .map(|&size| PackRef {
            root: [5; 32],
            size,
        })
        .collect();
    let decoded = broken(&wire::write(&entries, &packs));
    let built = Manifest::new(entries, packs).err().map(|e| rule_of(&e));
    assert_eq!(decoded, built, "the decoder and new disagree");
    decoded
}

const fn layout(sub: u8) -> Option<(u8, Option<u8>)> {
    Some((16, Some(sub)))
}

#[test]
fn rule_1_the_packs_are_used_in_order_from_the_first() {
    assert_eq!(
        verdict(vec![at("a", 0, 0, 5), at("b", 1, 0, 5)], &[5, 5]),
        None
    );
    assert_eq!(verdict(vec![at("a", 1, 0, 5)], &[5, 5]), layout(1));
    assert_eq!(
        verdict(vec![at("a", 0, 0, 5), at("b", 2, 0, 5)], &[5, 5, 5]),
        layout(1)
    );
    // Back to a pack that was left: a file of no bytes at the end of pack 0, after pack 1.
    let back = vec![at("a", 0, 0, 5), at("b", 1, 0, 5), at("c", 0, 5, 0)];
    assert_eq!(verdict(back, &[5, 5]), layout(1));
}

#[test]
fn rule_2_each_file_starts_where_the_one_before_it_ends() {
    assert_eq!(
        verdict(vec![at("a", 0, 0, 3), at("b", 0, 3, 4)], &[7]),
        None
    );
    // A gap, an overlap, and a pack whose first file does not start at 0.
    assert_eq!(
        verdict(vec![at("a", 0, 0, 3), at("b", 0, 4, 3)], &[7]),
        layout(2)
    );
    assert_eq!(
        verdict(vec![at("a", 0, 0, 3), at("b", 0, 2, 3)], &[5]),
        layout(2)
    );
    assert_eq!(verdict(vec![at("a", 0, 2, 3)], &[5]), layout(2));
    assert_eq!(
        verdict(vec![at("a", 0, 0, 5), at("b", 1, 1, 4)], &[5, 5]),
        layout(2)
    );
}

#[test]
fn rule_3_the_last_file_of_each_pack_ends_at_its_size() {
    assert_eq!(verdict(vec![at("a", 0, 0, 5)], &[5]), None);
    assert_eq!(verdict(vec![at("a", 0, 0, 3)], &[5]), layout(3));
    // The first pack is short where the second begins.
    assert_eq!(
        verdict(vec![at("a", 0, 0, 3), at("b", 1, 0, 5)], &[5, 5]),
        layout(3)
    );
}

#[test]
fn rule_4_no_pack_is_left_over() {
    assert_eq!(verdict(vec![typed_file("a")], &[]), None);
    assert_eq!(verdict(vec![typed_file("a")], &[5]), layout(4));
    assert_eq!(verdict(vec![at("a", 0, 0, 5)], &[5, 5]), layout(4));
}

#[test]
fn a_packed_file_of_no_bytes_at_its_place_breaks_nothing() {
    let entries = vec![at("a", 0, 0, 0), at("b", 0, 0, 5), at("c", 0, 5, 0)];
    assert_eq!(verdict(entries, &[5]), None);
}

#[test]
fn rule_5_no_pack_is_empty() {
    // The smallest pack there can be, and a pack of only files of no bytes, first and later.
    assert_eq!(verdict(vec![at("a", 0, 0, 1)], &[1]), None);
    assert_eq!(verdict(vec![at("a", 0, 0, 0)], &[0]), layout(5));
    let later = vec![at("a", 0, 0, 5), at("b", 1, 0, 0), at("c", 1, 0, 0)];
    assert_eq!(verdict(later, &[5, 0]), layout(5));
}

#[test]
fn blobs_folders_and_links_between_packed_files_change_nothing() {
    let entries = vec![
        at("a", 0, 0, 2),
        typed_dir("b"),
        typed_file("c"),
        typed_link("d", "x"),
        at("e", 0, 2, 3),
    ];
    assert_eq!(verdict(entries, &[5]), None);
}

#[test]
fn rule_14_is_checked_before_rule_16() {
    // Past the end of its pack, which breaks rule 16 as well.
    assert_eq!(verdict(vec![at("a", 0, 1, 5)], &[5]), Some((14, None)));
}
