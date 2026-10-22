use proptest::prelude::*;

use super::{DEFAULT_CAP_BYTES, Item, plan};

fn item(root: u8, size: u64, last_used_ms: u64) -> Item {
    Item {
        root: [root; 32],
        size,
        last_used_ms,
        tagged: false,
        in_use: false,
    }
}

fn roots(items: &[Item]) -> Vec<u8> {
    items.iter().map(|item| item.root[0]).collect()
}

#[test]
fn the_default_cap_is_10_gb_in_decimal() {
    assert_eq!(DEFAULT_CAP_BYTES, 10 * 1000 * 1000 * 1000);
}

#[test]
fn a_store_at_or_under_its_cap_loses_nothing() {
    for items in [
        vec![],
        vec![item(1, 0, 0)],
        vec![item(1, 60, 0), item(2, 40, 1)],
    ] {
        let plan = plan(&items, 100);
        assert!(plan.evict().is_empty() && plan.cap_met(), "{items:?}");
        assert_eq!(plan.freed(), 0);
        assert_eq!(plan.kept(), items.iter().map(|item| item.size).sum::<u64>());
    }
}

#[test]
fn the_least_recently_used_go_first_and_eviction_stops_as_soon_as_the_cap_is_met() {
    let items = [
        item(1, 100, 40),
        item(2, 100, 10),
        item(3, 100, 30),
        item(4, 100, 20),
    ];
    // 400 bytes, cap 250: two must go, the ones used at 10 and 20.
    let plan = plan(&items, 250);
    assert_eq!(roots(plan.evict()), [2, 4]);
    assert_eq!(
        (plan.freed(), plan.kept(), plan.cap_met()),
        (200, 200, true)
    );
    // A cap of exactly what two items take keeps both of them.
    assert_eq!(roots(super::plan(&items, 200).evict()), [2, 4]);
    assert_eq!(roots(super::plan(&items, 199).evict()), [2, 4, 3]);
}

#[test]
fn the_same_time_is_broken_by_root_so_the_plan_never_depends_on_input_order() {
    let items = [item(9, 10, 5), item(3, 10, 5), item(6, 10, 5)];
    assert_eq!(roots(plan(&items, 10).evict()), [3, 6]);
    let reversed: Vec<Item> = items.iter().rev().copied().collect();
    assert_eq!(plan(&reversed, 10), plan(&items, 10));
}

#[test]
fn tagged_and_in_use_items_are_never_evicted_however_old() {
    let tagged = Item {
        tagged: true,
        ..item(1, 100, 0)
    };
    let in_use = Item {
        in_use: true,
        ..item(2, 100, 1)
    };
    let plan = plan(&[tagged, in_use, item(3, 100, 99)], 200);
    assert_eq!(roots(plan.evict()), [3]);
    assert!(plan.cap_met());
}

#[test]
fn when_tagged_and_in_use_items_alone_pass_the_cap_every_other_item_goes() {
    let tagged = Item {
        tagged: true,
        ..item(1, 300, 0)
    };
    let in_use = Item {
        in_use: true,
        ..item(2, 300, 0)
    };
    let plan = plan(&[item(3, 10, 5), tagged, in_use, item(4, 10, 1)], 500);
    assert_eq!(roots(plan.evict()), [4, 3]);
    assert_eq!(
        (plan.freed(), plan.kept(), plan.cap_met()),
        (20, 600, false)
    );
}

#[test]
fn sizes_near_u64_max_do_not_overflow() {
    let items = [item(1, u64::MAX, 0), item(2, u64::MAX, 1), item(3, 5, 2)];
    let plan = plan(&items, u64::MAX);
    assert_eq!(roots(plan.evict()), [1, 2]);
    assert_eq!(
        (plan.freed(), plan.kept()),
        (u64::MAX, 5),
        "freed saturates"
    );
    let none = super::plan(&items, 0);
    assert_eq!(roots(none.evict()), [1, 2, 3]);
    assert_eq!(none.kept(), 0);
}

fn any_item() -> impl Strategy<Value = Item> {
    (
        0u8..16,
        0u64..1000,
        0u64..50,
        any::<bool>(),
        any::<bool>(),
        0u8..4,
    )
        .prop_map(|(root, size, last_used_ms, tagged, in_use, protect)| Item {
            root: [root; 32],
            size,
            last_used_ms,
            // Mostly items that may go, so most plans have something to do.
            tagged: tagged && protect == 0,
            in_use: in_use && protect == 1,
        })
}

proptest! {
    #[test]
    fn the_plan_keeps_its_promises(
        items in prop::collection::vec(any_item(), 0..40),
        cap in 0u64..20_000,
        seed in any::<u64>(),
    ) {
        let plan = plan(&items, cap);
        // Never a tagged or in-use item.
        prop_assert!(plan.evict().iter().all(Item::can_go));
        let protected: u64 = items.iter().filter(|i| !i.can_go()).map(|i| i.size).sum();
        let total: u64 = items.iter().map(|i| i.size).sum();
        prop_assert_eq!(plan.freed() + plan.kept(), total);
        // Under the cap whenever the cap can be met at all.
        prop_assert_eq!(plan.cap_met(), protected <= cap);
        if protected <= cap {
            prop_assert!(plan.kept() <= cap);
            // And not one more than needed: without the last one evicted, it would be over.
            if let Some(last) = plan.evict().last() {
                prop_assert!(plan.kept() + last.size > cap);
            }
        } else {
            prop_assert_eq!(plan.kept(), protected);
        }
        // Nothing evicted was used after something that may go and is kept.
        let mut left: Vec<Item> = items.iter().copied().filter(Item::can_go).collect();
        for gone in plan.evict() {
            let at = left.iter().position(|i| i == gone).expect("evicted from the input");
            left.remove(at);
        }
        for gone in plan.evict() {
            prop_assert!(left.iter().all(|kept| gone.last_used_ms <= kept.last_used_ms));
        }
        // The same plan for the same items in another order.
        let mut shuffled = items.clone();
        let mut state = seed;
        for at in (1..shuffled.len()).rev() {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            shuffled.swap(at, (state >> 33) as usize % (at + 1));
        }
        prop_assert_eq!(super::plan(&shuffled, cap), plan);
    }
}
