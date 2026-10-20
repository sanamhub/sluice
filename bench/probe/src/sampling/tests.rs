//! Tests for the sampling and path choice in the parent module.

use std::{sync::Arc, time::Instant};

use noq::congestion::{Bbr3Config, ControllerFactory, CubicConfig};

use super::*;
use crate::controller::ControllerReport;

fn path(remote_addr: &str) -> PathReport {
    PathReport {
        id: PathId::ZERO,
        selected: false,
        relay: false,
        remote_addr: remote_addr.to_string(),
        rtt_ms: 0.0,
        cwnd_bytes: 0,
        congestion_events: 0,
        spurious_congestion_events: 0,
        lost_packets: 0,
        lost_bytes: 0,
        sent_datagrams: 0,
        received_datagrams: 0,
        mtu: 0,
        controller: None,
    }
}

fn kept(at_s: u64, source: Source, remote_addr: &str) -> Option<Kept> {
    Some(Kept {
        at: Duration::from_secs(at_s),
        source,
        paths: vec![path(remote_addr)],
    })
}

#[test]
fn a_live_list_wins_and_is_stamped_with_the_report_time() {
    let now = Duration::from_millis(4_250);
    let kept = kept(3, Source::Stream, "kept");
    let (paths, at, source) = choose(vec![path("live")], now, kept);
    assert_eq!(paths[0].remote_addr, "live");
    assert_eq!(at, Some(4.25));
    assert_eq!(source, Source::Live);
}

#[test]
fn the_kept_list_stands_in_for_an_empty_live_one_with_its_own_time_and_source() {
    let kept = kept(3, Source::Sample, "kept");
    let (paths, at, source) = choose(Vec::new(), Duration::from_secs(4), kept);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].remote_addr, "kept");
    assert_eq!(at, Some(3.0));
    assert_eq!(source, Source::Sample);
}

#[test]
fn with_neither_there_are_no_paths_and_no_time() {
    let (paths, at, source) = choose(Vec::new(), Duration::from_secs(4), None);
    assert!(paths.is_empty());
    assert_eq!(at, None);
    assert_eq!(source, Source::None);
}

#[test]
fn the_newer_reading_is_kept_and_an_empty_one_is_ignored() {
    let mut series = Series::default();
    series.keep(Source::Sample, vec![path("a")], Duration::from_secs(1));
    series.keep(
        Source::Stream,
        vec![path("b")],
        Duration::from_millis(1_400),
    );
    series.keep(Source::Stream, Vec::new(), Duration::from_secs(2));
    let kept = series.kept.expect("a list was kept");
    assert_eq!(kept.source, Source::Stream);
    assert_eq!(kept.paths[0].remote_addr, "b");
    assert_eq!(kept.at, Duration::from_millis(1_400));
}

#[test]
fn a_sample_with_no_chosen_path_is_null_apart_from_its_time_and_the_path_counts() {
    let sample = reading(Duration::from_millis(3_013), 2, 2, None);
    assert_eq!(sample.elapsed_s, 3.013);
    assert_eq!(sample.open_paths, 2);
    assert_eq!(sample.stale_paths, 2);
    assert!(sample.path.is_none());
    assert!(sample.cwnd_bytes.is_none());
    assert!(sample.pacing_rate_bytes_s.is_none());
    assert!(sample.bbr_state.is_none());
    assert!(sample.bbr_max_bw_bytes_s.is_none());
    assert!(sample.bbr_app_limited.is_none());
}

#[test]
fn a_sample_takes_the_window_and_the_state_from_one_controller() {
    let bbr = Arc::new(Bbr3Config::default()).build(Instant::now(), 1452);
    let sample = reading(
        Duration::from_secs(1),
        1,
        0,
        Some((Chosen::Only, bbr.as_ref())),
    );
    assert_eq!(sample.path, Some(Chosen::Only));
    assert_eq!(sample.cwnd_bytes, Some(bbr.window()));
    assert_eq!(sample.bbr_state.as_deref(), Some("Startup"));

    let cubic = Arc::new(CubicConfig::default()).build(Instant::now(), 1452);
    let sample = reading(
        Duration::from_secs(1),
        1,
        0,
        Some((Chosen::Only, cubic.as_ref())),
    );
    assert_eq!(sample.cwnd_bytes, Some(cubic.window()));
    assert!(sample.bbr_state.is_none());
}

fn open(paths: &[(u32, bool, u64)]) -> Vec<Candidate> {
    paths
        .iter()
        .map(|&(id, selected, datagrams)| Candidate {
            id: PathId::from(id),
            selected,
            datagrams,
            live: true,
        })
        .collect()
}

/// `paths` with the one that has `id` stripped of its controller.
fn stale_at(mut paths: Vec<Candidate>, id: u32) -> Vec<Candidate> {
    paths
        .iter_mut()
        .filter(|c| c.id == PathId::from(id))
        .for_each(|c| c.live = false);
    paths
}

#[test]
fn the_selected_path_wins_over_a_busier_one() {
    let paths = open(&[(0, false, 900), (1, true, 10)]);
    assert_eq!(pick(&paths), Some((1, Chosen::Selected)));
}

#[test]
fn a_lone_path_is_the_only_one_selected_or_not() {
    assert_eq!(pick(&open(&[(0, false, 5)])), Some((0, Chosen::Only)));
    assert_eq!(pick(&open(&[(0, true, 5)])), Some((0, Chosen::Selected)));
}

#[test]
fn with_none_selected_the_path_with_the_most_datagrams_is_read() {
    let paths = open(&[(0, false, 40), (1, false, 700), (2, false, 90)]);
    assert_eq!(pick(&paths), Some((1, Chosen::Busiest)));
}

#[test]
fn a_tie_goes_to_the_lower_path_id_whatever_the_list_order() {
    let paths = open(&[(3, false, 50), (1, false, 50), (2, false, 50)]);
    assert_eq!(pick(&paths), Some((1, Chosen::Busiest)));
}

#[test]
fn no_open_path_is_no_choice() {
    assert_eq!(pick(&open(&[])), None);
}

#[test]
fn a_stale_selected_path_falls_through_to_the_live_one() {
    let paths = stale_at(open(&[(0, true, 900), (1, false, 10)]), 0);
    assert_eq!(pick(&paths), Some((1, Chosen::Only)));
}

#[test]
fn a_stale_path_is_not_the_busiest() {
    let paths = stale_at(open(&[(0, false, 900), (1, false, 10), (2, false, 20)]), 0);
    assert_eq!(pick(&paths), Some((2, Chosen::Busiest)));
}

#[test]
fn a_live_selected_path_still_wins_beside_a_stale_one() {
    let paths = stale_at(open(&[(0, false, 900), (1, true, 10)]), 0);
    assert_eq!(pick(&paths), Some((1, Chosen::Selected)));
}

#[test]
fn with_every_path_stale_there_is_no_choice() {
    let paths = stale_at(stale_at(open(&[(0, true, 5), (1, false, 9)]), 0), 1);
    assert_eq!(pick(&paths), None);
}

#[test]
fn stale_paths_are_counted_and_live_ones_are_not() {
    assert_eq!(stale(&open(&[(0, true, 1), (1, false, 1)])), 0);
    let paths = stale_at(open(&[(0, true, 1), (1, false, 1), (2, false, 1)]), 1);
    assert_eq!(stale(&paths), 1);
    assert_eq!(stale(&open(&[])), 0);
}

#[test]
fn the_report_summary_uses_the_same_choice() {
    let mut paths = vec![path("a"), path("b")];
    paths[0].sent_datagrams = 10;
    paths[1].received_datagrams = 20;
    paths[1].id = PathId::from(1u32);
    for p in &mut paths {
        p.controller = Some(controller_report());
    }
    assert_eq!(in_use(&paths).map(|p| p.remote_addr.as_str()), Some("b"));
    assert!(in_use(&[]).is_none());
}

#[test]
fn the_report_summary_skips_a_path_with_no_controller() {
    let mut paths = vec![path("stale"), path("live")];
    paths[0].selected = true;
    paths[1].id = PathId::from(1u32);
    paths[1].controller = Some(controller_report());
    assert_eq!(in_use(&paths).map(|p| p.remote_addr.as_str()), Some("live"));
}

fn controller_report() -> ControllerReport {
    let cubic = Arc::new(CubicConfig::default()).build(Instant::now(), 1452);
    controller::read(cubic.as_ref())
}
