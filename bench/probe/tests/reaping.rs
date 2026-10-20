//! A `fetch` or `serve` that outlives its test's deadline is killed, not left running.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    time::{Duration, Instant},
};

mod common;
use common::{Probes, probe, read_line, read_ticket, spawn_server, within_deadline_of};

const SHORT_DEADLINE: Duration = Duration::from_secs(3);

/// Runs `case` against `SHORT_DEADLINE` and returns what it started, after checking that the
/// deadline failed the test promptly.
fn past_the_deadline(case: impl FnOnce(&Probes) + Send + 'static) -> Arc<Probes> {
    let probes = Arc::new(Probes::default());
    let started = Instant::now();
    let gave_up = catch_unwind(AssertUnwindSafe(|| {
        within_deadline_of(SHORT_DEADLINE, &probes, case);
    }));
    assert!(gave_up.is_err(), "the deadline did not fail the test");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the reaper waited on a process that kept running"
    );
    probes
}

#[test]
fn a_fetch_past_the_deadline_is_killed_and_waited_for() {
    // No size moves in 600 seconds, which is `fetch`'s own default timeout, so only the reaper can
    // end it.
    let (_server, mut stdout) = spawn_server("raw", "bbr3", "64GiB", &[]);
    let ticket = read_ticket(&mut stdout);

    let probes = past_the_deadline(move |probes| {
        probes.fetch(probe().args(["fetch", &ticket, "--mode", "raw"]));
    });

    assert!(
        probes.all_reaped(),
        "a fetch is still running after its test"
    );
}

#[test]
fn a_serve_started_in_the_closure_is_killed_and_waited_for() {
    // The closure holds the `serve` guard, so a hung closure never drops it. Nothing connects, so
    // `serve` prints no second line and the read blocks until the deadline.
    let probes = past_the_deadline(|probes| {
        let (_serve, mut stdout) = probes.serve("raw", "bbr3", "8MiB", &[]);
        read_ticket(&mut stdout);
        read_line(&mut stdout);
    });

    assert!(
        probes.all_reaped(),
        "a serve is still running after its test"
    );
}
