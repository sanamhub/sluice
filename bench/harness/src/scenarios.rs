//! `scenarios.toml`: the scenarios M0 runs, from docs/performance.md.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// The whole file.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    scenario: Vec<Scenario>,
}

/// One row of the scenario table in docs/performance.md.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    /// `S1`, `S3` and so on.
    pub id: String,
    /// The link in words, copied into the summary.
    pub link: String,
    /// Absent for a link that is left as it is.
    pub netem: Option<Netem>,
    /// What is moved. The probe moves one blob, so a workload is a name and a size.
    pub workloads: Vec<Workload>,
    /// Seconds `fetch` may take before it reports a timeout.
    #[serde(default = "default_timeout")]
    pub timeout_s: u64,
    /// Seconds in a row below 1% of the run's busiest second before `fetch` is stopped early and
    /// the run is counted as stalled.
    #[serde(default = "default_stall")]
    pub stall_s: u64,
    /// Parallel connections for the `mscp` baseline.
    #[serde(default = "default_mscp_connections")]
    pub mscp_connections: u16,
    /// The probe configurations, each run once per workload.
    pub probe: Vec<ProbeConfig>,
}

fn default_timeout() -> u64 {
    600
}

fn default_stall() -> u64 {
    30
}

fn default_mscp_connections() -> u16 {
    4
}

/// What `tc netem` is asked for. Every field that is set is read back and checked.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Netem {
    /// Added delay in milliseconds.
    pub delay_ms: Option<f64>,
    /// Delay variation in milliseconds. Needs `delay_ms`.
    pub jitter_ms: Option<f64>,
    /// Loss as a percentage, so 0.1 is one packet in a thousand.
    pub loss_percent: Option<f64>,
    /// A `tc` rate such as `1gbit`.
    pub rate: Option<String>,
    /// Queue length. The `tc` default of 1000 packets drops on a long fat link.
    pub limit_packets: Option<u32>,
}

/// A named amount of data.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    /// Short name, used in the results.
    pub name: String,
    /// Size in the form `probe serve --size` takes, for example `100GiB`.
    pub size: String,
}

impl Workload {
    /// The size in bytes.
    pub fn bytes(&self) -> Result<u64> {
        parse_size(&self.size)
    }
}

/// Raw QUIC streams or an `iroh-blobs` get.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Streams only.
    Raw,
    /// Verified blobs.
    Blobs,
}

/// Congestion controller on both ends.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Congestion {
    /// BBRv3.
    Bbr3,
    /// Cubic.
    Cubic,
    /// NewReno.
    Newreno,
}

/// Flow control windows on both ends.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Window {
    /// What `noq` ships.
    Default,
    /// 75 000 000 bytes.
    Tuned,
}

/// Whether the connections of one fetch share a socket.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Endpoints {
    /// One socket each end.
    Shared,
    /// One socket per connection on both ends.
    Separate,
}

/// Where a fetched blob goes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sink {
    /// Verified and dropped.
    Discard,
    /// Written to a file store on the receiver.
    Store,
}

/// Where the provider keeps the blob.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Store {
    /// In memory.
    Mem,
    /// A file in the sender's data directory.
    Fs,
}

/// One row of the probe matrix.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeConfig {
    /// Raw or blobs.
    pub mode: Mode,
    /// Controller.
    pub congestion: Congestion,
    /// Windows.
    pub window: Window,
    /// Connections in the fetch.
    pub connections: u16,
    /// Socket sharing.
    pub endpoints: Endpoints,
    /// Blobs only. Discard if absent.
    pub sink: Option<Sink>,
    /// Blobs only. Memory if absent.
    pub store: Option<Store>,
}

impl ProbeConfig {
    /// The sink, which only a blobs fetch has.
    pub fn sink(&self) -> Option<Sink> {
        (self.mode == Mode::Blobs).then(|| self.sink.unwrap_or(Sink::Discard))
    }

    /// The store, which only a blobs provider has.
    pub fn store(&self) -> Option<Store> {
        (self.mode == Mode::Blobs).then(|| self.store.unwrap_or(Store::Mem))
    }

    /// True if the sender reads the blob from disk.
    pub fn reads_disk(&self) -> bool {
        self.store() == Some(Store::Fs)
    }

    /// True if the receiver writes the blob to disk.
    pub fn writes_disk(&self) -> bool {
        self.sink() == Some(Sink::Store)
    }

    /// A stable name for the configuration, used in the results.
    pub fn label(&self) -> String {
        let mut label = format!(
            "{} {} {} x{} {}",
            lower(&self.mode),
            lower(&self.congestion),
            lower(&self.window),
            self.connections,
            lower(&self.endpoints)
        );
        if let (Some(store), Some(sink)) = (self.store(), self.sink()) {
            label.push_str(&format!(" store={} sink={}", lower(&store), lower(&sink)));
        }
        label
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=16).contains(&self.connections),
            "connections must be 1 to 16"
        );
        if self.mode == Mode::Raw {
            ensure!(
                self.sink.is_none() && self.store.is_none(),
                "raw mode has no sink and no store"
            );
        }
        Ok(())
    }
}

/// The lower-case word serde uses for a unit variant.
fn lower<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}").to_lowercase()
}

/// Parses the file and checks what the types cannot.
pub fn parse(text: &str) -> Result<Vec<Scenario>> {
    let file: File = toml::from_str(text).context("parse scenarios")?;
    ensure!(!file.scenario.is_empty(), "no scenarios");
    for scenario in &file.scenario {
        let id = &scenario.id;
        ensure!(!scenario.workloads.is_empty(), "{id}: no workloads");
        ensure!(!scenario.probe.is_empty(), "{id}: no probe configurations");
        ensure!(scenario.stall_s >= 1, "{id}: stall_s must be at least 1");
        for workload in &scenario.workloads {
            workload
                .bytes()
                .with_context(|| format!("{id}: workload"))?;
        }
        for config in &scenario.probe {
            config
                .validate()
                .with_context(|| format!("{id}: {config:?}"))?;
        }
        if let Some(netem) = &scenario.netem {
            ensure!(
                netem.jitter_ms.is_none() || netem.delay_ms.is_some(),
                "{id}: jitter_ms needs delay_ms"
            );
        }
    }
    Ok(file.scenario)
}

/// Bytes in `text`: a number with an optional KiB, MiB, GiB or TiB suffix.
pub fn parse_size(text: &str) -> Result<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (digits, suffix) = text.split_at(split);
    let number: u64 = digits.parse().with_context(|| format!("size {text:?}"))?;
    let unit: u64 = match suffix {
        "" => 1,
        "KiB" => 1 << 10,
        "MiB" => 1 << 20,
        "GiB" => 1 << 30,
        "TiB" => 1 << 40,
        other => bail!("size {text:?}: unknown unit {other:?}"),
    };
    number
        .checked_mul(unit)
        .with_context(|| format!("size {text:?} is too large"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_committed_file_parses_and_covers_m0() {
        let scenarios = parse(include_str!("../../scenarios.toml")).expect("scenarios.toml");
        let ids: Vec<_> = scenarios.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["S1", "S3", "S6"]);
        assert!(scenarios[0].netem.is_none(), "S1 leaves the link alone");
        for scenario in &scenarios[1..] {
            assert!(scenario.netem.is_some(), "{} needs netem", scenario.id);
        }
        for scenario in &scenarios {
            let has = |congestion, window| {
                scenario
                    .probe
                    .iter()
                    .any(|p| p.congestion == congestion && p.window == window)
            };
            assert!(has(Congestion::Bbr3, Window::Default), "{}", scenario.id);
            assert!(has(Congestion::Bbr3, Window::Tuned), "{}", scenario.id);
            assert!(has(Congestion::Cubic, Window::Default), "{}", scenario.id);
        }
    }

    #[test]
    fn a_minimal_scenario_gets_the_defaults() {
        let text = r#"
            [[scenario]]
            id = "T"
            link = "test"
            workloads = [{ name = "w", size = "1MiB" }]
            probe = [
              { mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared" },
              { mode = "blobs", congestion = "cubic", window = "tuned", connections = 4, endpoints = "separate" },
            ]
        "#;
        let scenario = &parse(text).expect("parse")[0];
        assert_eq!(scenario.timeout_s, 600);
        assert_eq!(scenario.stall_s, 30);
        assert_eq!(scenario.mscp_connections, 4);
        let raw = &scenario.probe[0];
        assert_eq!((raw.sink(), raw.store()), (None, None));
        let blobs = &scenario.probe[1];
        assert_eq!(
            (blobs.sink(), blobs.store()),
            (Some(Sink::Discard), Some(Store::Mem))
        );
        assert_eq!(
            blobs.label(),
            "blobs cubic tuned x4 separate store=mem sink=discard"
        );
        assert!(!blobs.reads_disk() && !blobs.writes_disk());
    }

    #[test]
    fn a_raw_run_with_a_store_is_rejected() {
        let text = r#"
            [[scenario]]
            id = "T"
            link = "test"
            workloads = [{ name = "w", size = "1MiB" }]
            probe = [{ mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared", store = "fs" }]
        "#;
        let error = parse(text).expect_err("raw has no store");
        assert!(format!("{error:#}").contains("raw mode has no sink and no store"));
    }

    #[test]
    fn stall_s_can_be_set_and_cannot_be_zero() {
        let with = |stall: &str| {
            format!(
                r#"
                [[scenario]]
                id = "T"
                link = "test"
                {stall}
                workloads = [{{ name = "w", size = "1MiB" }}]
                probe = [{{ mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared" }}]
            "#
            )
        };
        assert_eq!(parse(&with("stall_s = 12")).expect("parse")[0].stall_s, 12);
        let error = parse(&with("stall_s = 0")).expect_err("zero would stop every run");
        assert!(format!("{error:#}").contains("stall_s"), "{error:#}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        let text = r#"
            [[scenario]]
            id = "T"
            link = "x"
            warp = 9
            workloads = [{ name = "w", size = "1MiB" }]
            probe = [{ mode = "raw", congestion = "bbr3", window = "default", connections = 1, endpoints = "shared" }]
        "#;
        let error = parse(text).expect_err("warp is not a key");
        assert!(format!("{error:#}").contains("warp"), "{error:#}");
    }

    #[test]
    fn sizes_take_binary_suffixes() {
        assert_eq!(parse_size("8MiB").unwrap(), 8 << 20);
        assert_eq!(parse_size("100GiB").unwrap(), 100 << 30);
        assert_eq!(parse_size("512").unwrap(), 512);
        assert!(parse_size("1GB").is_err());
        assert!(parse_size("GiB").is_err());
    }
}
