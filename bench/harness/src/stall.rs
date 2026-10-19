//! Spotting a collapsed transfer while it runs, from the progress lines `fetch` prints.

/// Fraction of the run's busiest second below which a second counts as quiet.
const QUIET: f64 = 0.01;

/// Follows the cumulative bytes `fetch` reports each second. A transfer has stalled when
/// `stall_s` seconds in a row moved less than 1% of the busiest second so far. The busiest second
/// so far is what a person watching would compare with, and a stall that starts early is caught
/// before the run could have seen a better second.
#[derive(Debug)]
pub struct StallWatch {
    stall_s: u64,
    previous: u64,
    peak: u64,
    quiet_in_a_row: u64,
}

impl StallWatch {
    /// A watch that fires after `stall_s` quiet seconds in a row.
    pub fn new(stall_s: u64) -> Self {
        Self {
            stall_s,
            previous: 0,
            peak: 0,
            quiet_in_a_row: 0,
        }
    }

    /// Takes the next second's cumulative bytes and returns true once the transfer has stalled.
    /// Nothing counts as quiet before any second has moved data, so a slow start is not a stall.
    pub fn push(&mut self, bytes: u64) -> bool {
        let moved = bytes.saturating_sub(self.previous);
        self.previous = bytes;
        let quiet = self.peak > 0 && (moved as f64) < QUIET * self.peak as f64;
        self.peak = self.peak.max(moved);
        self.quiet_in_a_row = if quiet { self.quiet_in_a_row + 1 } else { 0 };
        self.quiet_in_a_row >= self.stall_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fires_at(stall_s: u64, progress: &[u64]) -> Option<usize> {
        let mut watch = StallWatch::new(stall_s);
        progress.iter().position(|&bytes| watch.push(bytes))
    }

    #[test]
    fn a_steady_transfer_never_stalls() {
        let progress: Vec<u64> = (1..=100).map(|s| s * 1000).collect();
        assert_eq!(fires_at(3, &progress), None);
    }

    #[test]
    fn a_collapse_fires_after_the_stated_number_of_quiet_seconds() {
        // 1000 a second, then 1 a second: quiet from the 4th second on, and the 3rd quiet one fires.
        let progress = [1000, 2000, 3000, 3001, 3002, 3003, 3004];
        assert_eq!(fires_at(3, &progress), Some(5));
    }

    #[test]
    fn one_good_second_resets_the_count() {
        let progress = [1000, 2000, 2001, 2002, 3002, 3003, 3004, 4004];
        assert_eq!(fires_at(3, &progress), None);
    }

    #[test]
    fn exactly_one_percent_of_the_peak_is_not_quiet() {
        // The peak is 1000, so 10 is 1% and 9 is under it.
        assert_eq!(fires_at(2, &[1000, 1010, 1020]), None);
        assert_eq!(fires_at(2, &[1000, 1009, 1018]), Some(2));
    }

    #[test]
    fn a_transfer_that_has_not_started_is_not_stalled() {
        assert_eq!(fires_at(2, &[0, 0, 0, 0, 0]), None);
        // It is judged against the peak once there is one.
        assert_eq!(fires_at(2, &[0, 0, 1000, 1000, 1000]), Some(4));
    }

    #[test]
    fn the_comparison_is_with_the_busiest_second_so_far() {
        // A slow but steady transfer is not a stall: every second is the busiest second so far.
        let progress: Vec<u64> = (1..=60).map(|s| s * 10).collect();
        assert_eq!(fires_at(5, &progress), None);
    }

    #[test]
    fn a_window_of_one_second_fires_on_the_first_quiet_second() {
        assert_eq!(fires_at(1, &[1000, 1000]), Some(1));
    }
}
