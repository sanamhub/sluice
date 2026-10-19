//! `bench run --size`: one size for every workload, with a record of what it replaced.

use anyhow::{Result, ensure};
use serde::Serialize;

use crate::scenarios::{Scenario, parse_size};

/// A workload whose committed size was replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Replaced {
    /// `S1` and so on.
    pub scenario: String,
    /// The workload's name.
    pub workload: String,
    /// The size in `scenarios.toml`.
    pub was: String,
}

/// What `--size` changed, for `summary.md` and `raw.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SizeOverride {
    /// The size every workload got.
    pub size: String,
    /// What each workload had before.
    pub replaced: Vec<Replaced>,
}

impl SizeOverride {
    /// `20GiB instead of S1 one-file 100GiB, S3 one-file 10GiB`.
    pub fn describe(&self) -> String {
        let was: Vec<String> = self
            .replaced
            .iter()
            .map(|r| format!("{} {} {}", r.scenario, r.workload, r.was))
            .collect();
        format!("{} instead of {}", self.size, was.join(", "))
    }
}

/// Sets every workload of `scenarios` to `size` and returns what it replaced. The committed
/// sizes are the published scenarios, so a result with another size has to say so.
pub fn apply(scenarios: &mut [Scenario], size: &str) -> Result<SizeOverride> {
    ensure!(parse_size(size)? > 0, "--size must be more than zero");
    let mut replaced = Vec::new();
    for scenario in scenarios {
        for workload in &mut scenario.workloads {
            replaced.push(Replaced {
                scenario: scenario.id.clone(),
                workload: workload.name.clone(),
                was: std::mem::replace(&mut workload.size, size.to_string()),
            });
        }
    }
    Ok(SizeOverride {
        size: size.to_string(),
        replaced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenarios;

    fn committed() -> Vec<Scenario> {
        scenarios::parse(include_str!("../../scenarios.toml")).expect("scenarios.toml")
    }

    #[test]
    fn every_workload_gets_the_size_and_the_old_ones_are_kept() {
        let mut all = committed();
        let changed = apply(&mut all, "20GiB").expect("a valid size");
        assert!(
            all.iter()
                .flat_map(|s| &s.workloads)
                .all(|w| w.size == "20GiB")
        );
        let was: Vec<(&str, &str)> = changed
            .replaced
            .iter()
            .map(|r| (r.scenario.as_str(), r.was.as_str()))
            .collect();
        assert_eq!(was, [("S1", "100GiB"), ("S3", "10GiB"), ("S6", "50GiB")]);
        assert_eq!(
            changed.describe(),
            "20GiB instead of S1 one-file 100GiB, S3 one-file 10GiB, S6 one-file 50GiB"
        );
    }

    #[test]
    fn a_size_the_probe_would_refuse_is_refused_before_anything_runs() {
        let mut all = committed();
        assert!(apply(&mut all, "20GB").is_err());
        assert!(apply(&mut all, "0GiB").is_err());
        assert!(apply(&mut all, "").is_err());
        // Nothing changed.
        assert_eq!(all[0].workloads[0].size, "100GiB");
    }

    #[test]
    fn only_the_scenarios_passed_in_are_listed() {
        let mut all = committed();
        all.retain(|s| s.id == "S3");
        let changed = apply(&mut all, "1GiB").expect("a valid size");
        assert_eq!(changed.replaced.len(), 1);
        assert_eq!(changed.replaced[0].scenario, "S3");
    }
}
