//! `cargo xtask cold-start`: times how long `sluice` takes to start and finish, against the 30 ms
//! budget in `docs/performance.md`.
//!
//! The commands are `sluice id` with a key that already exists, and `sluice --version`, which does
//! no work of ours. The second is the floor: on Windows most of the time is the system starting a
//! process, and the difference between the two is what `id` itself costs. Timing is from Rust, so
//! no `hyperfine` is needed on the CI runners.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde_json::Value;

/// The budget for a median: cold start to a printed result.
const BUDGET: Duration = Duration::from_millis(30);
/// Runs that are timed and runs that are thrown away first, so the files are in the cache.
const RUNS: usize = 30;
const WARM_UP: usize = 3;

pub fn run() -> anyhow::Result<()> {
    let binary = build_release()?;
    let home = tempfile::tempdir().context("cannot make a temporary home")?;
    // The first run makes the key, so every timed run loads an existing one.
    time_once(&binary, home.path(), &["id"])?;

    let mut over = Vec::new();
    for (label, args) in [("sluice id", ["id"]), ("sluice --version", ["--version"])] {
        let times = measure(&binary, home.path(), &args)?;
        let (median, max) = (median(&times), *times.iter().max().expect("RUNS is not 0"));
        println!(
            "{label:<18} median {:>6.1} ms   max {:>6.1} ms   ({RUNS} runs, {WARM_UP} warm-ups)",
            millis(median),
            millis(max)
        );
        if median > BUDGET {
            over.push(label);
        }
    }
    println!("budget             median under {} ms", BUDGET.as_millis());
    if !over.is_empty() {
        bail!(
            "median over the {} ms budget: {}",
            BUDGET.as_millis(),
            over.join(", ")
        );
    }
    Ok(())
}

/// Builds `sluice` in release and returns the path cargo reports for it, so the target folder
/// does not have to be guessed.
fn build_release() -> anyhow::Result<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["build", "--release", "-p", "sluice-cli"])
        .args(["--message-format", "json-render-diagnostics"])
        .stderr(Stdio::inherit())
        .output()
        .context("cannot run cargo")?;
    if !output.status.success() {
        bail!("cargo build --release -p sluice-cli failed");
    }
    executable_in(&String::from_utf8_lossy(&output.stdout))
        .context("cargo did not report the sluice binary")
}

/// The `executable` of the `sluice` binary in cargo's JSON messages, one per line.
fn executable_in(messages: &str) -> Option<PathBuf> {
    messages
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message["reason"] == "compiler-artifact")
        .filter(|message| message["target"]["name"] == "sluice")
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
}

fn measure(binary: &Path, home: &Path, args: &[&str]) -> anyhow::Result<Vec<Duration>> {
    for _ in 0..WARM_UP {
        time_once(binary, home, args)?;
    }
    (0..RUNS).map(|_| time_once(binary, home, args)).collect()
}

/// Starts the binary and waits for it, with its output thrown away so printing does not count.
fn time_once(binary: &Path, home: &Path, args: &[&str]) -> anyhow::Result<Duration> {
    let start = Instant::now();
    let status = Command::new(binary)
        .env("SLUICE_HOME", home)
        .env_remove("NO_COLOR")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("cannot run {}", binary.display()))?;
    let elapsed = start.elapsed();
    if !status.success() {
        bail!("`sluice {}` exited with {status}", args.join(" "));
    }
    Ok(elapsed)
}

/// The middle of the sorted times, or the mean of the two middle ones for an even count.
fn median(times: &[Duration]) -> Duration {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2
    }
}

fn millis(time: Duration) -> f64 {
    time.as_secs_f64() * 1000.0
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::{executable_in, median};

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().map(|&v| Duration::from_millis(v)).collect()
    }

    #[test]
    fn the_median_is_the_middle_value_and_the_mean_of_two_for_an_even_count() {
        assert_eq!(median(&ms(&[9, 1, 5])), Duration::from_millis(5));
        assert_eq!(
            median(&ms(&[4, 1, 3, 10])),
            Duration::from_millis(3) + Duration::from_micros(500)
        );
        assert_eq!(median(&ms(&[7])), Duration::from_millis(7));
    }

    #[test]
    fn the_binary_is_found_among_cargos_messages() {
        let messages = [
            r#"{"reason":"compiler-artifact","target":{"name":"sluice_core"},"executable":null}"#,
            r#"{"reason":"compiler-artifact","target":{"name":"sluice"},"executable":"/t/sluice"}"#,
            r#"{"reason":"build-finished","success":true}"#,
            "not json",
        ]
        .join("\n");
        assert_eq!(executable_in(&messages), Some(PathBuf::from("/t/sluice")));
        assert_eq!(executable_in("not json"), None);
    }
}
