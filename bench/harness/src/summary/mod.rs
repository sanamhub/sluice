//! `summary.md`: per scenario and configuration, in plain tables.

use std::fmt::Write;

use crate::{
    netem::Shown,
    stats::{is_slow, median, outliers, percent_of, spread, worst_dip},
};

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
    /// What `tc qdisc show` printed on each side, or `None` for an unshaped link.
    pub netem: Option<Shown>,
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
    let dip = worst_dip(measured.iter().filter_map(|r| r.progress.as_deref()))
        .map_or("n/a".to_string(), two);
    let late = if ok.is_empty() {
        "n/a".to_string()
    } else {
        outliers(&ok).to_string()
    };
    format!(
        "| {} | {} | {} / {} | {} | {} | {} | {} | {} | {} | {} | {} |",
        row.workload,
        row.name,
        count(Status::Ok),
        row.runs.len(),
        count(Status::Timeout),
        count(Status::Failed),
        slow,
        dip,
        late,
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
                "netem on both interfaces, as `tc qdisc show` read them back:\n\n\
                 - sender: `{}`\n- receiver: `{}`",
                shown.sender.trim(),
                shown.receiver.trim()
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
        "| workload | configuration | ok / runs | timeouts | failed | slow | worst dip | outliers | median Gbit/s | min to max | % of lower ceiling |"
    );
    let _ = writeln!(
        out,
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"
    );
    for row in &s.rows {
        let _ = writeln!(out, "{}", render_row(row, &s.ceilings));
    }
    let _ = writeln!(
        out,
        "\nWorst dip is the lowest second of a run over the median second of the same run, leaving out \
         the first and last second, as the lowest across the runs, and an outlier is a finished run \
         that took more than 1.5 times as long as the fastest finished run."
    );
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
mod tests;
