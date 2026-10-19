use super::*;
use crate::size_override::Replaced;

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
    // under a tenth of the peak: the one at 201 and the stalled timeout. The dip leaves out
    // the first and last second, so every run with a middle is steady or has no ratio, and
    // 6 Gbit/s is exactly 1.5 times 4, which is not an outlier.
    assert_eq!(
        line,
        "| one-file | raw bbr3 default x1 shared | 3 / 4 | 1 | 0 | 0 | 2 | 1.00 | 0 | 5.00 | 4.00 to 6.00 | 50% |"
    );
}

#[test]
fn a_dip_and_a_late_run_that_the_ten_percent_rule_misses_show_in_their_own_columns() {
    // The second run has a second at exactly a tenth of its peak, which is not slow, but it is
    // a tenth of its typical second. It also took three times as long as the first.
    let row = Row {
        workload: "one-file".to_string(),
        name: "raw bbr3 default x1 shared".to_string(),
        reads_disk: false,
        writes_disk: false,
        runs: vec![
            run(Status::Ok, 6.0, &[100, 200, 300, 400, 500]),
            run(Status::Ok, 2.0, &[100, 200, 210, 310, 410]),
        ],
    };
    let line = render_row(&row, &Ceilings::default());
    assert_eq!(
        line,
        "| one-file | raw bbr3 default x1 shared | 2 / 2 | 0 | 0 | 0 | 0 | 0.10 | 1 | 4.00 | 2.00 to 6.00 | n/a |"
    );
}

#[test]
fn a_row_with_no_finished_run_has_no_outlier_count() {
    let row = Row {
        runs: vec![run(Status::Timeout, 0.0, &[100, 100, 100, 100])],
        ..row(false, false)
    };
    let line = render_row(&row, &Ceilings::default());
    assert!(
        line.contains("| 0 / 1 | 1 | 0 | 0 | 1 | n/a | n/a | n/a | n/a | n/a |"),
        "{line}"
    );
}

#[test]
fn a_stalled_run_is_counted_on_its_own_and_not_as_a_timeout() {
    let row = Row {
        runs: vec![
            run(Status::Ok, 4.0, &[100, 200, 300, 400]),
            run(Status::Stalled, 0.0, &[100, 200, 201, 202]),
            run(Status::Timeout, 0.0, &[100, 100, 100, 100]),
        ],
        ..row(false, false)
    };
    let line = render_row(&row, &Ceilings::default());
    // The stalled run's live progress counts toward slow runs like any other: two of the three.
    assert!(line.contains("| 1 / 3 | 1 | 1 | 0 | 2 |"), "{line}");
}

#[test]
fn a_row_without_a_ceiling_says_so() {
    let line = render_row(&row(false, false), &Ceilings::default());
    assert!(line.ends_with("| n/a |"), "{line}");
}

fn scenario(netem: Option<Shown>, skipped: Option<&str>, rows: Vec<Row>) -> ScenarioReport {
    ScenarioReport {
        id: "S1".to_string(),
        link: "LAN".to_string(),
        netem,
        skipped: skipped.map(str::to_string),
        ceilings: Ceilings::default(),
        rows,
        not_measured: vec!["iperf3: missing on the receiver".to_string()],
    }
}

fn report(scenarios: Vec<ScenarioReport>) -> Report {
    Report {
        label: "lab".to_string(),
        date: "2026-10-19".to_string(),
        hosts: vec!["sender: local (linux)".to_string()],
        runs: 5,
        size_override: None,
        scenarios,
    }
}

#[test]
fn the_summary_has_a_table_per_scenario() {
    let text = render(&report(vec![
        scenario(None, None, vec![row(false, false)]),
        scenario(None, Some("netem needs Linux"), Vec::new()),
    ]));
    assert!(text.starts_with("# Bench results: lab"));
    assert!(text.contains("## S1: LAN"));
    assert!(text.contains("| ceiling | runs | median | min to max |"));
    assert!(text.contains("| slow | worst dip | outliers | median Gbit/s |"));
    assert!(text.contains("Worst dip is the lowest second of a run"));
    assert!(text.contains("- iperf3: missing on the receiver"));
    assert!(text.contains("Skipped: netem needs Linux"));
}

#[test]
fn an_overridden_size_is_stated_with_what_it_replaced() {
    let mut r = report(Vec::new());
    assert!(!render(&r).contains("Size overridden"));
    r.size_override = Some(SizeOverride {
        size: "20GiB".to_string(),
        replaced: vec![
            Replaced {
                scenario: "S1".to_string(),
                workload: "one-file".to_string(),
                was: "100GiB".to_string(),
            },
            Replaced {
                scenario: "S3".to_string(),
                workload: "one-file".to_string(),
                was: "10GiB".to_string(),
            },
        ],
    });
    let text = render(&r);
    assert!(text.contains(
        "- Size overridden with `--size`: 20GiB instead of S1 one-file 100GiB, S3 one-file 10GiB. \
         These are not the committed sizes."
    ));
}

#[test]
fn a_shaped_scenario_shows_what_each_side_read_back() {
    let shown = Shown {
        sender: "qdisc netem 8001: root limit 20000 delay 40ms loss 0.1% rate 1Gbit\n".to_string(),
        receiver: "qdisc netem 8001: root limit 1000 delay 40ms\n".to_string(),
    };
    let text = render(&report(vec![scenario(Some(shown), None, Vec::new())]));
    assert!(text.contains(
        "- sender: `qdisc netem 8001: root limit 20000 delay 40ms loss 0.1% rate 1Gbit`"
    ));
    assert!(text.contains("- receiver: `qdisc netem 8001: root limit 1000 delay 40ms`"));
}
