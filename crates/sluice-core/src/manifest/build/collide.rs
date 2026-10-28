//! Names that a receiver would take for one: two that are the same after NFC, and two that are the
//! same after case folding. `Manifest::new` would refuse the first one it meets. This lists all.

use std::collections::HashMap;

use super::walk::Item;
use super::{Finding, Reason};
use crate::manifest::path::fold_key;

/// Every item that shares its path with another under the one key a name is compared by, each with
/// its own path as the sender sees it.
pub(super) fn find(items: &[Item]) -> Vec<Finding> {
    let mut groups: HashMap<String, Vec<&Item>> = HashMap::new();
    for item in items {
        groups
            .entry(fold_key(item.path.as_str()))
            .or_default()
            .push(item);
    }
    let mut found = Vec::new();
    for group in groups.values().filter(|group| group.len() > 1) {
        for item in group {
            // Two names that are one path are the same path to the letter, so they are told apart
            // from names that only differ in case, which the user fixes differently.
            let twin = group
                .iter()
                .any(|other| !std::ptr::eq(*other, *item) && other.path == item.path);
            found.push(Finding {
                path: item.shown.clone(),
                reason: if twin {
                    Reason::SameAfterNfc
                } else {
                    Reason::CaseCollision
                },
            });
        }
    }
    found
}
