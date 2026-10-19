//! The state one `bench run` shares between the measuring steps.

use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};

use crate::{host::Host, results::Results, tools::Tool};

/// What a run does besides the probe.
#[derive(Clone, Copy, Debug)]
pub struct Plan {
    /// False for `bench smoke`, which uses the probe only and records the other tools as missing
    /// or skipped.
    pub ceilings_and_baseline: bool,
}

/// Both hosts, what they have, and where the results go.
pub struct Session {
    /// Runs `probe serve`.
    pub sender: Host,
    /// Runs `probe fetch`, and is the one netem shapes.
    pub receiver: Host,
    /// What the sender has.
    pub sender_tools: Vec<Tool>,
    /// What the receiver has.
    pub receiver_tools: Vec<Tool>,
    /// `raw.jsonl` and the directory around it.
    pub results: Results,
    /// Runs per configuration.
    pub runs: u32,
    /// What to run.
    pub plan: Plan,
    /// Runs the harness could not get a result from, or refused.
    pub failed: u32,
}

impl Session {
    /// Appends one object to `raw.jsonl`.
    pub fn record(&mut self, value: &Value) -> Result<()> {
        self.results.record(value)
    }

    /// Records a tool that did not run, with the reason, and returns the line for the summary.
    pub fn record_not_run(
        &mut self,
        scenario: &str,
        kind: &str,
        status: &str,
        reason: &str,
    ) -> Result<String> {
        self.record(&json!({
            "kind": kind,
            "scenario": scenario,
            "run": null,
            "status": status,
            "reason": reason,
        }))?;
        Ok(format!("{kind}: {reason}"))
    }
}

/// A duration long enough for a command that moves `bytes` at a pessimistic 50 MB/s, with a floor
/// of five minutes. Used to bound commands whose length the harness does not control.
pub fn generous(bytes: u64) -> Duration {
    Duration::from_secs((bytes / 50_000_000).max(300))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_jobs_get_more_time_than_short_ones() {
        assert_eq!(generous(1 << 20), Duration::from_secs(300));
        assert_eq!(generous(100 << 30), Duration::from_secs(2147));
    }
}
