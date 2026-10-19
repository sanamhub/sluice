//! A congestion controller as far as `noq`'s public API shows it.
//!
//! `Controller::metrics` gives the window, the pacing rate and the send quantum for any
//! controller. BBR's own state is not in that trait and `Bbr3` has no public accessors, so for BBR
//! the rest is read from its derived `Debug` output. A field that is missing or does not parse is
//! `null`, and `noq` is pinned, so a format change shows up as nulls and in the test below.

use noq::congestion::Controller;
use serde::Serialize;

/// What one path's controller reported when it was read.
#[derive(Serialize)]
pub struct ControllerReport {
    /// Bytes per second the pacer is allowed to send at. `null` if the controller does not pace.
    pub pacing_rate_bytes_s: Option<u64>,
    /// Largest burst the pacer may release at once.
    pub send_quantum_bytes: Option<u64>,
    /// Slow start threshold. `null` for controllers without one, such as BBR, and for Cubic until
    /// its first congestion event, when `noq` reports `u64::MAX` for "not set".
    pub ssthresh_bytes: Option<u64>,
    /// Bytes BBR counts as in flight. `null` for other controllers, which expose no such count.
    pub bytes_in_flight: Option<u64>,
    /// BBR's state machine, for example `Startup` or `ProbeBw(Cruise)`. `null` for other
    /// controllers.
    pub bbr_state: Option<String>,
    /// BBR's bottleneck bandwidth estimate in bytes per second. `null` for other controllers.
    pub bbr_max_bw_bytes_s: Option<f64>,
    /// Whether BBR currently counts the sender as application-limited. `null` for other
    /// controllers.
    pub bbr_app_limited: Option<bool>,
}

/// Reads `controller` without changing it.
pub fn read(controller: &dyn Controller) -> ControllerReport {
    let metrics = controller.metrics();
    let debug = format!("{controller:?}");
    let bbr = debug.starts_with("Bbr3 {").then(|| fields(&debug));
    let field = |name: &str| {
        bbr.as_ref()?
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
    };
    ControllerReport {
        pacing_rate_bytes_s: metrics.pacing_rate,
        send_quantum_bytes: metrics.send_quantum,
        ssthresh_bytes: metrics.ssthresh.filter(|&s| s != u64::MAX),
        bytes_in_flight: field("inflight").and_then(|v| v.parse().ok()),
        bbr_state: field("state").map(str::to_string),
        bbr_max_bw_bytes_s: field("max_bw").and_then(|v| v.parse().ok()),
        // `Bbr3::app_limited` is the delivered count that ends the phase, and 0 outside it.
        bbr_app_limited: field("app_limited")
            .and_then(|v| v.parse::<u64>().ok())
            .map(|n| n != 0),
    }
}

/// The top-level `name: value` pairs of a derived `Debug` struct. Nested structs, tuples and lists
/// stay whole in the value, so a field of the same name inside one cannot be mistaken for it.
fn fields(debug: &str) -> Vec<(&str, &str)> {
    let Some(open) = debug.find('{') else {
        return Vec::new();
    };
    let body = &debug[open + 1..debug.rfind('}').unwrap_or(debug.len())];
    let mut pieces = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in body.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                pieces.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&body[start..]);
    pieces
        .into_iter()
        .filter_map(|piece| piece.split_once(':'))
        .map(|(name, value)| (name.trim(), value.trim()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use noq::congestion::{Bbr3Config, ControllerFactory, CubicConfig};

    use super::{fields, read};

    #[test]
    fn nested_fields_are_not_mistaken_for_top_level_ones() {
        let parsed = fields("Thing { a: 1, inner: Inner { a: 2, b: [3, 4] }, c: Some((5, 6)) }");
        assert_eq!(
            parsed,
            [
                ("a", "1"),
                ("inner", "Inner { a: 2, b: [3, 4] }"),
                ("c", "Some((5, 6))")
            ]
        );
    }

    /// A fresh `Bbr3` is in `Startup` with nothing in flight and no bandwidth estimate. This also
    /// fails if a `noq` upgrade renames a field this module reads.
    #[test]
    fn a_new_bbr3_reads_as_startup() {
        let bbr = Arc::new(Bbr3Config::default()).build(Instant::now(), 1452);
        let report = read(bbr.as_ref());
        assert_eq!(report.bbr_state.as_deref(), Some("Startup"));
        assert_eq!(report.bytes_in_flight, Some(0));
        assert_eq!(report.bbr_max_bw_bytes_s, Some(0.0));
        assert_eq!(report.bbr_app_limited, Some(false));
        assert!(report.pacing_rate_bytes_s.is_some());
        assert!(report.send_quantum_bytes.is_some());
    }

    #[test]
    fn cubic_has_no_bbr_fields() {
        let cubic = Arc::new(CubicConfig::default()).build(Instant::now(), 1452);
        let report = read(cubic.as_ref());
        assert!(report.bbr_state.is_none() && report.bytes_in_flight.is_none());
        assert!(report.bbr_max_bw_bytes_s.is_none() && report.bbr_app_limited.is_none());
        assert!(report.pacing_rate_bytes_s.is_none());
        assert!(report.ssthresh_bytes.is_none());
    }
}
