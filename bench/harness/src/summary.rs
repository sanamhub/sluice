//! `summary.md`: per scenario and configuration, in plain tables.

use std::fmt::Write;

use crate::stats::{is_slow, median, percent_of, spread};

/// How one run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Every byte arrived.
    Ok,
    /// `fetch` hit its timeout. A result, but not a throughput.
    Timeout,
    /// The harness could not get a result, or refused it.
    Failed,
}

/// One run, reduced to what the summary uses.
#[derive(Clone, Debug)]
pub struct RunResult {
    /// How it ended.
    pub status: Status,
    /// Payload throughput in Gbit/s, for a finished run.
    pub gbit_s: Option<f64>,
    /// Cumulative bytes per second, if the tool reports it. `mscp` does not.
    pub progress: Option<Vec<u64>>,
}

/// One configuration under one workload, with its runs.
#[derive(Clone, Debug)]
pub struct Row {
    /// Workload name.
    pub workload: String,
    /// Configuration label.
    pub name: String,
    /// The sender reads the data from disk.
    pub reads_disk: bool,
    /// The receiver writes the data to disk.
    pub writes_disk: bool,
    /// The runs, in order.
    pub runs: Vec<RunResult>,
}

/// What the path and the disks can do, from the ceiling tools. One entry per run.
#[derive(Clone, Debug, Default)]
pub struct Ceilings {
    /// `iperf3` with one stream, bits per second.
    pub iperf_single: Vec<f64>,
    /// `iperf3` with parallel streams, bits per second.
    pub iperf_parallel: Vec<f64>,
    /// `fio` sequential read on the sender, bytes per second.
    pub fio_read: Vec<f64>,
    /// `fio` sequential write on the receiver, bytes per second.
    pub fio_write: Vec<f64>,
}

impl Ceilings {
    /// The lower of the ceilings a configuration is bounded by, in Gbit/s: the network, and the
    /// disks it touches. `None` if any of those was not measured, because a lower bound with a
    /// hole in it would flatter the result.
    pub fn lower_for(&self, row: &Row) -> Option<f64> {
        let gbit = |bits: f64| bits / 1e9;
        let bytes = |b: f64| b * 8.0 / 1e9;
        let network = [median(&self.iperf_single), median(&self.iperf_parallel)]
            .into_iter()
            .flatten()
            .max_by(f64::total_cmp)
            .map(gbit)?;
        let mut lower = network;
        if row.reads_disk {
            lower = lower.min(bytes(median(&self.fio_read)?));
        }
        if row.writes_disk {
            lower = lower.min(bytes(median(&self.fio_write)?));
        }
        Some(lower)
    }
}

/// Everything about one scenario.
#[derive(Clone, Debug)]
pub struct ScenarioReport {
    /// `S1` and so on.
    pub id: String,
    /// The link in words.
    pub link: String,
    /// What `tc qdisc show` printed, or `None` for an unshaped link.
    pub netem: Option<String>,
    /// Why nothing ran, if nothing did.
    pub skipped: Option<String>,
    /// Ceilings measured on this path.
    pub ceilings: Ceilings,
    /// Probe and `mscp` rows.
    pub rows: Vec<Row>,
    /// Tools that did not run, with the reason.
    pub not_measured: Vec<String>,
}

/// The whole run.
#[derive(Clone, Debug)]
pub struct Report {
    /// Label given on the command line.
    pub label: String,
    /// UTC date.
    pub date: String,
    /// `sender` and `receiver` as described in the hosts file.
    pub hosts: Vec<String>,
    /// Runs per configuration.
    pub runs: u32,
    /// The scenarios, in file order.
    pub scenarios: Vec<ScenarioReport>,
}

fn two(value: f64) -> String {
    format!("{value:.2}")
}

fn range(values: &[f64], unit: f64) -> String {
    match spread(values) {
        Some((min, max)) => format!("{} to {}", two(min / unit), two(max / unit)),
        None => "n/a".to_string(),
    }
}

fn ceiling_line(name: &str, values: &[f64], unit: f64) -> String {
    let runs = values.len();
    match median(values) {
        Some(m) => format!(
            "| {name} | {runs} | {} | {} |",
            two(m / unit),
            range(values, unit)
        ),
        None => format!("| {name} | 0 | n/a | n/a |"),
    }
}

fn render_row(row: &Row, ceilings: &Ceilings) -> String {
    let count = |s: Status| row.runs.iter().filter(|r| r.status == s).count();
    let ok: Vec<f64> = row
        .runs
        .iter()
        .filter(|r| r.status == Status::Ok)
        .filter_map(|r| r.gbit_s)
        .collect();
    let measured: Vec<&RunResult> = row.runs.iter().filter(|r| r.progress.is_some()).collect();
    let slow = if measured.is_empty() {
        "n/a".to_string()
    } else {
        let n = measured
            .iter()
            .filter(|r| r.progress.as_deref().is_some_and(is_slow))
            .count();
        n.to_string()
    };
    let percent = median(&ok)
        .zip(ceilings.lower_for(row))
        .and_then(|(m, ceiling)| percent_of(m, ceiling))
        .map_or("n/a".to_string(), |p| format!("{p:.0}%"));
    format!(
        "| {} | {} | {} / {} | {} | {} | {} | {} | {} | {} |",
        row.workload,
        row.name,
        count(Status::Ok),
        row.runs.len(),
        count(Status::Timeout),
        count(Status::Failed),
        slow,
        median(&ok).map_or("n/a".to_string(), two),
        range(&ok, 1.0),
        percent
    )
}

fn render_scenario(out: &mut String, s: &ScenarioReport) {
    let _ = writeln!(out, "\n## {}: {}\n", s.id, s.link);
    if let Some(reason) = &s.skipped {
        let _ = writeln!(out, "Skipped: {reason}");
        return;
    }
    match &s.netem {
        Some(shown) => {
            let _ = writeln!(
                out,
                "netem on the receiver's interface, as `tc qdisc show` read it back: `{}`",
                shown.trim()
            );
        }
        None => {
            let _ = writeln!(out, "No netem. The link is left as it is.");
        }
    }
    let _ = writeln!(out, "\n### Ceilings\n");
    let _ = writeln!(out, "| ceiling | runs | median | min to max |");
    let _ = writeln!(out, "| --- | --- | --- | --- |");
    let c = &s.ceilings;
    let lines = [
        ceiling_line("iperf3, 1 stream (Gbit/s)", &c.iperf_single, 1e9),
        ceiling_line("iperf3, parallel streams (Gbit/s)", &c.iperf_parallel, 1e9),
        ceiling_line(
            "fio read on the sender (Gbit/s)",
            &bytes_to_bits(&c.fio_read),
            1e9,
        ),
        ceiling_line(
            "fio write on the receiver (Gbit/s)",
            &bytes_to_bits(&c.fio_write),
            1e9,
        ),
    ];
    for line in lines {
        let _ = writeln!(out, "{line}");
    }
    let _ = writeln!(out, "\n### Results\n");
    let _ = writeln!(
        out,
        "| workload | configuration | ok / runs | timeouts | failed | slow | median Gbit/s | min to max | % of lower ceiling |"
    );
    let _ = writeln!(
        out,
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for row in &s.rows {
        let _ = writeln!(out, "{}", render_row(row, &s.ceilings));
    }
    if !s.not_measured.is_empty() {
        let _ = writeln!(out, "\nNot measured:\n");
        for line in &s.not_measured {
            let _ = writeln!(out, "- {line}");
        }
    }
}

fn bytes_to_bits(values: &[f64]) -> Vec<f64> {
    values.iter().map(|b| b * 8.0).collect()
}

/// The summary as Markdown.
pub fn render(report: &Report) -> String {
    let mut out = format!("# Bench results: {}\n\n", report.label);
    let _ = writeln!(out, "- Date (UTC): {}", report.date);
    for host in &report.hosts {
        let _ = writeln!(out, "- {host}");
    }
    let _ = writeln!(out, "- Runs per configuration: {}", report.runs);
    let _ = writeln!(
        out,
        "\nA run is slow if any whole second moved less than a tenth of the run's busiest second. \
         The median covers finished runs only, and a timeout is counted, not averaged in."
    );
    for scenario in &report.scenarios {
        render_scenario(&mut out, scenario);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: Status, gbit_s: f64, progress: &[u64]) -> RunResult {
        RunResult {
            status,
            gbit_s: (status == Status::Ok).then_some(gbit_s),
            progress: Some(progress.to_vec()),
        }
    }

    fn row(reads_disk: bool, writes_disk: bool) -> Row {
        Row {
            workload: "one-file".to_string(),
            name: "raw bbr3 default x1 shared".to_string(),
            reads_disk,
            writes_disk,
            runs: vec![
                run(Status::Ok, 4.0, &[100, 200, 300]),
                run(Status::Ok, 5.0, &[100, 200, 300]),
                run(Status::Ok, 6.0, &[100, 200, 201]),
                run(Status::Timeout, 0.0, &[100, 100, 100]),
            ],
        }
    }

    #[test]
    fn the_lower_ceiling_covers_the_network_and_the_disks_a_row_touches() {
        let ceilings = Ceilings {
            iperf_single: vec![9.0e9],
            iperf_parallel: vec![9.4e9, 9.5e9, 9.3e9],
            fio_read: vec![1.0e9],
            fio_write: vec![0.5e9],
        };
        // The network ceiling is the better of the two iperf3 medians: 9.4 Gbit/s.
        assert_eq!(ceilings.lower_for(&row(false, false)), Some(9.4));
        // 1 GB/s is 8 Gbit/s and 0.5 GB/s is 4 Gbit/s.
        assert_eq!(ceilings.lower_for(&row(true, false)), Some(8.0));
        assert_eq!(ceilings.lower_for(&row(true, true)), Some(4.0));
    }

    #[test]
    fn a_missing_ceiling_leaves_no_percentage() {
        let no_disk = Ceilings {
            iperf_parallel: vec![10.0e9],
            ..Ceilings::default()
        };
        assert_eq!(no_disk.lower_for(&row(false, false)), Some(10.0));
        assert_eq!(no_disk.lower_for(&row(true, false)), None);
        assert_eq!(Ceilings::default().lower_for(&row(false, false)), None);
    }

    #[test]
    fn a_row_reports_the_median_spread_slow_runs_and_percent() {
        let ceilings = Ceilings {
            iperf_parallel: vec![10.0e9],
            ..Ceilings::default()
        };
        let line = render_row(&row(false, false), &ceilings);
        // Three finished runs of 4, 5 and 6 Gbit/s, one timeout, and two runs with a second
        // under a tenth of the peak: the one at 201 and the stalled timeout.
        assert_eq!(
            line,
            "| one-file | raw bbr3 default x1 shared | 3 / 4 | 1 | 0 | 2 | 5.00 | 4.00 to 6.00 | 50% |"
        );
    }

    #[test]
    fn a_row_without_a_ceiling_says_so() {
        let line = render_row(&row(false, false), &Ceilings::default());
        assert!(line.ends_with("| n/a |"), "{line}");
    }

    #[test]
    fn the_summary_has_a_table_per_scenario() {
        let report = Report {
            label: "lab".to_string(),
            date: "2026-10-19".to_string(),
            hosts: vec!["sender: local (linux)".to_string()],
            runs: 5,
            scenarios: vec![
                ScenarioReport {
                    id: "S1".to_string(),
                    link: "LAN".to_string(),
                    netem: None,
                    skipped: None,
                    ceilings: Ceilings::default(),
                    rows: vec![row(false, false)],
                    not_measured: vec!["iperf3: missing on the receiver".to_string()],
                },
                ScenarioReport {
                    id: "S3".to_string(),
                    link: "WAN".to_string(),
                    netem: None,
                    skipped: Some("netem needs Linux".to_string()),
                    ceilings: Ceilings::default(),
                    rows: Vec::new(),
                    not_measured: Vec::new(),
                },
            ],
        };
        let text = render(&report);
        assert!(text.starts_with("# Bench results: lab"));
        assert!(text.contains("## S1: LAN"));
        assert!(text.contains("| ceiling | runs | median | min to max |"));
        assert!(text.contains("- iperf3: missing on the receiver"));
        assert!(text.contains("Skipped: netem needs Linux"));
    }
}
