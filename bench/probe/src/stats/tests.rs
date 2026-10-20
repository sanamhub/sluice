//! Tests for the report's choice of `unlisted_path_ids` in the parent module.

use super::*;

/// Holds a controller for each id in `held`, as a connection whose `noq` state is that.
fn holding(held: &'static [u32]) -> impl Fn(PathId) -> bool {
    move |id| held.iter().any(|&n| PathId::from(n) == id)
}

/// A path iroh lists, with the id the report compares by.
fn listed(id: u32) -> PathReport {
    PathReport {
        id: PathId::from(id),
        ..PathReport::default()
    }
}

#[test]
fn an_empty_list_reports_every_id_noq_holds() {
    // The first shape: iroh listed nothing and `noq` had paths 0 and 1.
    assert_eq!(unlisted_path_ids(&[], holding(&[0, 1])), [0, 1]);
}

#[test]
fn a_listed_path_noq_dropped_is_not_hiding_the_one_it_holds() {
    // The second shape: iroh still lists path 0, `noq` dropped it and carries the data on path 1.
    assert_eq!(unlisted_path_ids(&[listed(0)], holding(&[1])), [1]);
}

#[test]
fn a_listed_path_noq_holds_leaves_nothing_unlisted() {
    assert!(unlisted_path_ids(&[listed(0)], holding(&[0])).is_empty());
}

#[test]
fn a_listed_id_is_not_asked_of_noq() {
    let ids = unlisted_path_ids(&[listed(3)], |id| {
        assert_ne!(id, PathId::from(3u32), "noq was asked about a listed path");
        false
    });
    assert!(ids.is_empty());
}

#[test]
fn an_empty_list_and_no_controller_is_empty() {
    assert!(unlisted_path_ids(&[], holding(&[])).is_empty());
}

#[test]
fn ids_are_found_up_to_the_limit() {
    let last = UNLISTED_ID_LIMIT - 1;
    assert_eq!(
        unlisted_path_ids(&[], |id| id == PathId::from(last)),
        [last]
    );
    assert!(unlisted_path_ids(&[], |id| id == PathId::from(last + 1)).is_empty());
}
