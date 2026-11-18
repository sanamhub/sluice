//! The provider's two short-lived locks. Both are poisoned only by a panic that changed nothing
//! they guard, so a poisoned lock is taken rather than propagated.

use std::sync::Mutex;

use crate::engine::policy;

/// The places lock, poisoned only by a panic that changed nothing it guards: a holder edits a set
/// and reads a clock.
pub(super) fn lock_places(
    places: &Mutex<policy::Places>,
) -> std::sync::MutexGuard<'_, policy::Places> {
    places
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The refused-group lock, poisoned only by a panic that changed nothing it guards: it edits a set
/// of endpoint ids.
pub(super) fn lock_lines(lines: &Mutex<policy::Lines>) -> std::sync::MutexGuard<'_, policy::Lines> {
    lines
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
