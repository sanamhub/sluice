//! The summary of a `bench connect` run: for each port mapping setting, how often the two hosts
//! connected with a rendezvous server and nothing else.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
    net::{IpAddr, SocketAddr},
};

use crate::{rendezvous::PortMapping, stats::median};

/// How one attempt ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ended {
    /// A connection came up. With no relay, that is a direct one. The transfer may still have run
    /// out of time.
    Connected,
    /// The dial failed or did not finish in time.
    NoPath,
    /// No result line, or an error in the harness or the probe.
    Other,
}

/// One `serve` and `fetch` pair.
#[derive(Clone, Debug)]
pub struct Attempt {
    pub port_mapping: PortMapping,
    pub ended: Ended,
    /// Seconds from the dial to the connection being up.
    pub connect_s: Option<f64>,
    /// What the rendezvous server told the sender.
    pub sender_learned: Vec<String>,
    /// What the rendezvous server told the receiver.
    pub receiver_learned: Vec<String>,
}

/// Everything that goes into `summary.md`.
pub struct Report {
    pub label: String,
    pub date: String,
    pub hosts: Vec<String>,
    pub rendezvous: String,
    pub runs: u32,
    pub attempts: Vec<Attempt>,
}

/// One row: the attempts that had this port mapping setting.
fn row(setting: PortMapping, attempts: &[Attempt]) -> String {
    let mine: Vec<&Attempt> = attempts
        .iter()
        .filter(|a| a.port_mapping == setting)
        .collect();
    let count = |ended: Ended| mine.iter().filter(|a| a.ended == ended).count();
    let times: Vec<f64> = mine.iter().filter_map(|a| a.connect_s).collect();
    let seconds = |value: Option<f64>| value.map_or("n/a".to_string(), |s| format!("{s:.2}"));
    let slowest = times.iter().copied().max_by(f64::total_cmp);
    let sender: BTreeSet<&String> = mine.iter().flat_map(|a| &a.sender_learned).collect();
    let receiver: BTreeSet<&String> = mine.iter().flat_map(|a| &a.receiver_learned).collect();
    format!(
        "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        setting.word(),
        mine.len(),
        count(Ended::Connected),
        count(Ended::NoPath),
        count(Ended::Other),
        seconds(median(&times)),
        seconds(slowest),
        addresses(&sender),
        addresses(&receiver)
    )
}

fn addresses(set: &BTreeSet<&String>) -> String {
    if set.is_empty() {
        return "none".to_string();
    }
    // Every attempt gets a new source port from the NAT, so listing sockets would fill the table with
    // one cell per run. The ports are in raw.jsonl.
    let mut ports: BTreeMap<IpAddr, BTreeSet<u16>> = BTreeMap::new();
    let mut unparsed = Vec::new();
    for address in set {
        match address.parse::<SocketAddr>() {
            Ok(socket) => {
                ports.entry(socket.ip()).or_default().insert(socket.port());
            }
            Err(_) => unparsed.push((*address).clone()),
        }
    }
    let mut cells: Vec<String> = ports
        .into_iter()
        .map(|(ip, ports)| match ports.first() {
            Some(port) if ports.len() == 1 => format!("`{}`", SocketAddr::new(ip, *port)),
            _ => format!("`{ip}` ({} ports)", ports.len()),
        })
        .collect();
    cells.extend(unparsed.into_iter().map(|a| format!("`{a}`")));
    cells.join(", ")
}

/// The summary as Markdown.
pub fn render(report: &Report) -> String {
    let mut out = format!("# Connection results: {}\n\n", report.label);
    let _ = writeln!(out, "- Date (UTC): {}", report.date);
    for host in &report.hosts {
        let _ = writeln!(out, "- {host}");
    }
    let _ = writeln!(out, "- Rendezvous: {}", report.rendezvous);
    let _ = writeln!(
        out,
        "- Runs per setting: {}, with the two settings interleaved",
        report.runs
    );
    let _ = writeln!(
        out,
        "\nEach attempt starts `serve` on the sender and `fetch` on the receiver, both with the \
         rendezvous server and no relay, and moves 16 MiB over one raw stream with a 30 s timeout. \
         Connected means a connection came up, and with no relay that is a direct one. `no_path` is a \
         dial that failed or did not finish in 30 s. `other` is an attempt with no result line. \
         `connect_s` runs from the dial to the connection being up, after both ends have waited up \
         to 10 s for the server's answer.\n"
    );
    let _ = writeln!(
        out,
        "| port mapping | attempts | connected | no_path | other | median connect_s | slowest connect_s | sender learnt | receiver learnt |"
    );
    let _ = writeln!(
        out,
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for setting in [PortMapping::Off, PortMapping::On] {
        let _ = writeln!(out, "{}", row(setting, &report.attempts));
    }
    let _ = writeln!(
        out,
        "\n`learnt` is every public IP address the rendezvous server reported for that end, each listed \
         once, with how many different ports it reported for it. `raw.jsonl` has every socket address. \
         One rendezvous server cannot tell whether a NAT gives a different public port for each \
         destination, because that takes a second server to compare against. These numbers say how \
         often the two hosts connected, and they do not name a NAT type."
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attempt(
        setting: PortMapping,
        ended: Ended,
        connect_s: Option<f64>,
        sender: &str,
    ) -> Attempt {
        Attempt {
            port_mapping: setting,
            ended,
            connect_s,
            sender_learned: vec![sender.to_string()],
            receiver_learned: vec!["198.51.100.4:5000".to_string()],
        }
    }

    #[test]
    fn a_row_counts_each_ending_and_times_the_connected_attempts() {
        let attempts = [
            attempt(
                PortMapping::Off,
                Ended::Connected,
                Some(0.4),
                "203.0.113.9:1",
            ),
            attempt(
                PortMapping::Off,
                Ended::Connected,
                Some(2.0),
                "203.0.113.9:2",
            ),
            attempt(PortMapping::Off, Ended::NoPath, None, "203.0.113.9:1"),
            attempt(PortMapping::Off, Ended::Other, None, "203.0.113.9:1"),
            attempt(
                PortMapping::On,
                Ended::Connected,
                Some(0.1),
                "203.0.113.9:3",
            ),
        ];
        assert_eq!(
            row(PortMapping::Off, &attempts),
            "| off | 4 | 2 | 1 | 1 | 1.20 | 2.00 | `203.0.113.9` (2 ports) | `198.51.100.4:5000` |"
        );
        assert_eq!(
            row(PortMapping::On, &attempts),
            "| on | 1 | 1 | 0 | 0 | 0.10 | 0.10 | `203.0.113.9:3` | `198.51.100.4:5000` |"
        );
    }

    #[test]
    fn addresses_are_grouped_by_ip_and_ipv6_keeps_its_brackets() {
        let wanted = [
            "203.0.113.9:1",
            "203.0.113.9:2",
            "198.51.100.4:7",
            "[2001:db8::1]:9",
            "odd",
        ]
        .map(String::from);
        let set: BTreeSet<&String> = wanted.iter().collect();
        assert_eq!(
            addresses(&set),
            "`198.51.100.4:7`, `203.0.113.9` (2 ports), `[2001:db8::1]:9`, `odd`"
        );
    }

    #[test]
    fn a_setting_with_no_attempt_has_no_times_and_no_addresses() {
        assert_eq!(
            row(PortMapping::On, &[]),
            "| on | 0 | 0 | 0 | 0 | n/a | n/a | none | none |"
        );
    }

    #[test]
    fn the_summary_says_what_one_rendezvous_cannot_tell() {
        let report = Report {
            label: "lab".to_string(),
            date: "2026-10-20".to_string(),
            hosts: vec!["sender: local (Linux)".to_string()],
            rendezvous: "https://rendezvous.example.net".to_string(),
            runs: 2,
            attempts: Vec::new(),
        };
        let text = render(&report);
        assert!(text.contains("do not name a NAT type"));
        assert!(text.contains("second server to compare against"));
        assert!(text.contains("| port mapping | attempts |"));
        assert!(text.contains("| off | 0 |") && text.contains("| on | 0 |"));
    }
}
