//! A `fetch` that outlives its test's deadline is killed, not left running.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::{Duration, Instant},
};

mod common;
use common::{Fetches, probe, read_ticket, spawn_server, within_deadline_of};

#[test]
fn a_fetch_past_the_deadline_is_killed_and_waited_for() {
    // No size moves in 600 seconds, which is `fetch`'s own default timeout, so only the reaper can
    // end it.
    let (_server, mut stdout) = spawn_server("raw", "bbr3", "64GiB", &[]);
    let ticket = read_ticket(&mut stdout);
    let fetches = Arc::new(Fetches::default());

    let started = Instant::now();
    let gave_up = catch_unwind(AssertUnwindSafe(|| {
        within_deadline_of(Duration::from_secs(3), &fetches, move |fetches| {
            fetches.run(probe().args(["fetch", &ticket, "--mode", "raw"]));
        });
    }));

    assert!(gave_up.is_err(), "the deadline did not fail the test");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the fetch ran on"
    );
    assert!(
        fetches.all_reaped(),
        "a fetch is still running after its test"
    );
}
