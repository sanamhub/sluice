//! What `sluice gc` would remove from the store, as `docs/design.md` (**Storage**) sets it: the
//! store keeps at most a cap of bytes, 10 GB by default, and goes over it by evicting what was
//! used least recently, never what is tagged or in use.
//!
//! This is the decision only. It is a pure function of the items and the cap, so
//! `sluice gc --dry-run` and the real run plan the same thing, and nothing here touches a file.

#[cfg(test)]
mod tests;

/// The cap of `docs/design.md`: 10 GB, decimal, as the design writes it.
///
/// # Examples
///
/// ```
/// use sluice_core::reclaim::DEFAULT_CAP_BYTES;
///
/// assert_eq!(DEFAULT_CAP_BYTES, 10_000_000_000);
/// ```
pub const DEFAULT_CAP_BYTES: u64 = 10_000_000_000;

/// One piece of content in the store.
///
/// # Examples
///
/// ```
/// use sluice_core::reclaim::Item;
///
/// let item = Item {
///     root: [7; 32],
///     size: 4096,
///     last_used_ms: 1_800_000_000_000,
///     tagged: false,
///     in_use: false,
/// };
/// assert!(item.can_go());
/// assert!(!Item { tagged: true, ..item }.can_go());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    /// The BLAKE3 root of the content.
    pub root: [u8; 32],
    /// What it takes on disk, outboard included. The caller adds it up.
    pub size: u64,
    /// When it was last used, in UTC milliseconds since the Unix epoch.
    pub last_used_ms: u64,
    /// Whether the user tagged it to keep. A tagged item is never evicted.
    pub tagged: bool,
    /// Whether a job in the queue that is not history refers to it. It is never evicted, because
    /// that job would fetch it again.
    pub in_use: bool,
}

impl Item {
    /// Whether the plan may evict this item: it is neither tagged nor in use.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::reclaim::Item;
    ///
    /// let item = Item { root: [0; 32], size: 1, last_used_ms: 0, tagged: false, in_use: true };
    /// assert!(!item.can_go());
    /// ```
    pub fn can_go(&self) -> bool {
        !self.tagged && !self.in_use
    }
}

/// What to evict to bring the store under its cap, and what that leaves.
///
/// # Examples
///
/// ```
/// use sluice_core::reclaim::{Item, plan};
///
/// let item = |root, last_used_ms| Item {
///     root: [root; 32],
///     size: 600,
///     last_used_ms,
///     tagged: false,
///     in_use: false,
/// };
/// let plan = plan(&[item(1, 30), item(2, 10), item(3, 20)], 1000);
/// // 1800 bytes and a cap of 1000: the two used least recently go, oldest first.
/// let gone: Vec<u8> = plan.evict().iter().map(|item| item.root[0]).collect();
/// assert_eq!(gone, [2, 3]);
/// assert_eq!((plan.freed(), plan.kept(), plan.cap_met()), (1200, 600, true));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    evict: Vec<Item>,
    freed: u64,
    kept: u64,
    cap_met: bool,
}

impl Plan {
    /// The items to evict, in the order they go: least recently used first, and for the same
    /// time by root, so one input always gives one plan.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::reclaim::{Item, plan};
    ///
    /// let item = Item { root: [1; 32], size: 10, last_used_ms: 0, tagged: false, in_use: false };
    /// assert_eq!(plan(&[item], 5).evict(), [item]);
    /// ```
    pub fn evict(&self) -> &[Item] {
        &self.evict
    }

    /// The bytes the evicted items take.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::reclaim::{Item, plan};
    ///
    /// let item = Item { root: [1; 32], size: 10, last_used_ms: 0, tagged: false, in_use: false };
    /// assert_eq!(plan(&[item], 5).freed(), 10);
    /// ```
    pub fn freed(&self) -> u64 {
        self.freed
    }

    /// The bytes the store holds after the plan, tagged and in-use items included.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::reclaim::{Item, plan};
    ///
    /// let item = Item { root: [1; 32], size: 10, last_used_ms: 0, tagged: true, in_use: false };
    /// assert_eq!(plan(&[item], 5).kept(), 10);
    /// ```
    pub fn kept(&self) -> u64 {
        self.kept
    }

    /// Whether what is kept is at most the cap. It is not when the tagged and in-use items alone
    /// take more than the cap: those are never evicted, so the plan evicts every other item and
    /// says the cap cannot be met.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::reclaim::{Item, plan};
    ///
    /// let tagged = Item { root: [1; 32], size: 10, last_used_ms: 0, tagged: true, in_use: false };
    /// let other = Item { root: [2; 32], tagged: false, ..tagged };
    /// let plan = plan(&[tagged, other], 5);
    /// assert!(!plan.cap_met());
    /// assert_eq!(plan.evict(), [other]);
    /// ```
    pub fn cap_met(&self) -> bool {
        self.cap_met
    }
}

/// The plan for a store of `items` and a cap of `cap_bytes`. Items that may go are evicted least
/// recently used first until what the store keeps is at most the cap, and not one more. Items
/// that are tagged or in use are never evicted.
///
/// # Examples
///
/// ```
/// use sluice_core::reclaim::{DEFAULT_CAP_BYTES, Item, plan};
///
/// let small = Item {
///     root: [1; 32],
///     size: 1 << 20,
///     last_used_ms: 0,
///     tagged: false,
///     in_use: false,
/// };
/// let plan = plan(&[small], DEFAULT_CAP_BYTES);
/// assert!(plan.evict().is_empty() && plan.cap_met());
/// ```
pub fn plan(items: &[Item], cap_bytes: u64) -> Plan {
    // Sums in u128, so no set of sizes can overflow them.
    let total: u128 = items.iter().map(|item| u128::from(item.size)).sum();
    let mut candidates: Vec<Item> = items.iter().copied().filter(Item::can_go).collect();
    candidates.sort_unstable_by_key(|item| (item.last_used_ms, item.root, item.size));
    let cap = u128::from(cap_bytes);
    let mut kept = total;
    let mut evict = Vec::new();
    for item in candidates {
        if kept <= cap {
            break;
        }
        kept -= u128::from(item.size);
        evict.push(item);
    }
    Plan {
        freed: saturate(total - kept),
        kept: saturate(kept),
        cap_met: kept <= cap,
        evict,
    }
}

fn saturate(bytes: u128) -> u64 {
    u64::try_from(bytes).unwrap_or(u64::MAX)
}
