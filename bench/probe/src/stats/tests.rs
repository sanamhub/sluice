//! Tests for the report's choice of `unlisted_path_ids` in the parent module.

use super::*;

/// Holds a controller for each id in `held`, as a connection whose `noq` state is that.
fn holding(held: &'static [u32]) -> impl Fn(PathId) -> bool {
    move |id| held.iter().any(|&n| PathId::from(n) == id)
}

#[test]
fn no_listed_path_reports_the_ids_noq_holds() {
    // The shape of the failure the field exists for: iroh listed nothing, `noq` had paths 0 and 1.
    assert_eq!(unlisted_path_ids(&[], holding(&[0, 1])), [0, 1]);
}

#[test]
fn ids_beyond_the_first_few_are_found_up_to_the_limit() {
    let last = UNLISTED_ID_LIMIT - 1;
    assert_eq!(
        unlisted_path_ids(&[], |id| id == PathId::from(last)),
        [last]
    );
    assert!(unlisted_path_ids(&[], |id| id == PathId::from(last + 1)).is_empty());
}

#[test]
fn a_listed_path_leaves_the_field_empty_without_asking_noq() {
    let listed = [PathReport::default()];
    let ids = unlisted_path_ids(&listed, |_| {
        panic!("noq was asked although iroh listed a path")
    });
    assert!(ids.is_empty());
}

#[test]
fn no_listed_path_and_no_controller_is_empty() {
    assert!(unlisted_path_ids(&[], holding(&[])).is_empty());
}
