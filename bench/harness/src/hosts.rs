//! The hosts file: who sends, who receives, and where things live on each.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// The whole file. Not committed, because it names real machines.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hosts {
    /// The host that runs `probe serve`.
    pub sender: Entry,
    /// The host that runs `probe fetch`, and whose interface netem shapes.
    pub receiver: Entry,
}

/// One end of the path.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// `local`, or an ssh destination such as `bench@receiver.example.net`.
    pub host: String,
    /// The network interface of the benchmark path. netem shapes the receiver's, and either
    /// one is described in `env.json`.
    pub interface: Option<String>,
    /// Where the other host reaches this one for `iperf3` and `mscp`. Defaults to the host part
    /// of `host`, and to 127.0.0.1 for `local`.
    pub address: Option<String>,
    /// Path of the `probe` binary on this host.
    pub probe: String,
    /// A directory this user can write. The harness keeps its files in it and never deletes
    /// anything it did not create.
    pub data_dir: String,
}

impl Entry {
    /// The address the other end uses to reach this host.
    pub fn reachable_at(&self) -> String {
        if let Some(address) = &self.address {
            return address.clone();
        }
        if self.host == "local" {
            return "127.0.0.1".to_string();
        }
        match self.host.rsplit_once('@') {
            Some((_, host)) => host.to_string(),
            None => self.host.clone(),
        }
    }
}

/// Reads and checks the hosts file at `path`.
pub fn load(path: &Path) -> Result<Hosts> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    parse(&text).with_context(|| format!("parse {}", path.display()))
}

/// Parses the text of a hosts file.
pub fn parse(text: &str) -> Result<Hosts> {
    let hosts: Hosts = toml::from_str(text)?;
    for (role, entry) in [("sender", &hosts.sender), ("receiver", &hosts.receiver)] {
        ensure!(!entry.host.trim().is_empty(), "{role}: host is empty");
        ensure!(!entry.probe.is_empty(), "{role}: probe is empty");
        ensure!(!entry.data_dir.is_empty(), "{role}: data_dir is empty");
        // A destination that starts with a dash would be read by ssh as an option.
        ensure!(
            !entry.host.starts_with('-'),
            "{role}: host must not start with a dash"
        );
    }
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_example_file_parses() {
        let hosts = parse(include_str!("../../hosts.example.toml")).expect("hosts.example.toml");
        assert_eq!(hosts.sender.host, "bench@sender.example.net");
        assert!(hosts.receiver.interface.is_some());
    }

    #[test]
    fn the_address_comes_from_the_host_unless_named() {
        let entry = |host: &str, address: Option<&str>| Entry {
            host: host.to_string(),
            interface: None,
            address: address.map(str::to_string),
            probe: "probe".to_string(),
            data_dir: "/data".to_string(),
        };
        assert_eq!(entry("local", None).reachable_at(), "127.0.0.1");
        assert_eq!(entry("bench@box.lan", None).reachable_at(), "box.lan");
        assert_eq!(entry("box", None).reachable_at(), "box");
        assert_eq!(entry("box", Some("10.0.0.2")).reachable_at(), "10.0.0.2");
    }

    #[test]
    fn a_host_that_looks_like_an_option_is_rejected() {
        let text = "[sender]\nhost = \"-oProxyCommand=x\"\nprobe = \"p\"\ndata_dir = \"d\"\n\
                    [receiver]\nhost = \"local\"\nprobe = \"p\"\ndata_dir = \"d\"\n";
        assert!(parse(text).is_err());
    }
}
