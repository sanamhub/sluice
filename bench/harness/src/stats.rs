//! The arithmetic behind `summary.md`: median, spread, slow runs, dips, outliers and percent of a
//! ceiling.

/// The middle value, or the mean of the two middle values of an even count. `None` if empty.
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mid = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[mid]
    } else {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    })
}

/// The smallest and the largest value. The spread of five runs is these two, so one outlier is
/// visible and not averaged away.
pub fn spread(values: &[f64]) -> Option<(f64, f64)> {
    let min = values.iter().copied().min_by(f64::total_cmp)?;
    let max = values.iter().copied().max_by(f64::total_cmp)?;
    Some((min, max))
}

/// The payload each second moved. `progress` is the cumulative payload at the end of each second,
/// as `fetch` prints it.
fn per_second(progress: &[u64]) -> Vec<u64> {
    let mut previous = 0;
    progress
        .iter()
        .map(|&total| {
            let moved = total.saturating_sub(previous);
            previous = total;
            moved
        })
        .collect()
}

/// True if any whole second moved less than a tenth of what the run's busiest second moved.
pub fn is_slow(progress: &[u64]) -> bool {
    let seconds = per_second(progress);
    let Some(&peak) = seconds.iter().max() else {
        return false;
    };
    seconds
        .iter()
        .any(|&moved| (moved as f64) < 0.1 * peak as f64)
}

/// The lowest second of a run divided by the run's median second, leaving out the first second,
/// which is still ramping up, and the last, which the end of the transfer cuts short. `None` if
/// fewer than three seconds remain, or if the median second moved nothing and so there is no
/// ratio.
pub fn dip(progress: &[u64]) -> Option<f64> {
    let seconds = per_second(progress);
    if seconds.len() < 3 {
        return None;
    }
    let inner: Vec<f64> = seconds[1..seconds.len() - 1]
        .iter()
        .map(|&moved| moved as f64)
        .collect();
    let typical = median(&inner).filter(|m| *m > 0.0)?;
    let lowest = inner.iter().copied().min_by(f64::total_cmp)?;
    Some(lowest / typical)
}

/// The deepest dip among the runs.
pub fn worst_dip<'a>(runs: impl IntoIterator<Item = &'a [u64]>) -> Option<f64> {
    runs.into_iter().filter_map(dip).min_by(f64::total_cmp)
}

/// How many finished runs took more than 1.5 times as long as the fastest one. All runs of a
/// configuration move the same bytes, so a run's time is in inverse proportion to its Gbit/s.
pub fn outliers(gbit_s: &[f64]) -> usize {
    let Some(fastest) = gbit_s.iter().copied().max_by(f64::total_cmp) else {
        return 0;
    };
    gbit_s.iter().filter(|&&g| g * 1.5 < fastest).count()
}

/// `value` as a percentage of `ceiling`. `None` if there is no usable ceiling.
pub fn percent_of(value: f64, ceiling: f64) -> Option<f64> {
    (ceiling > 0.0).then(|| value / ceiling * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_of_an_odd_count_is_the_middle_value() {
        assert_eq!(median(&[5.0, 1.0, 3.0]), Some(3.0));
        assert_eq!(median(&[4.1, 4.0, 9.0, 4.2, 4.3]), Some(4.2));
    }

    #[test]
    fn the_median_of_an_even_count_is_the_mean_of_the_middle_two() {
        assert_eq!(median(&[1.0, 2.0, 10.0, 4.0]), Some(3.0));
    }

    #[test]
    fn nothing_has_no_median_and_no_spread() {
        assert_eq!(median(&[]), None);
        assert_eq!(spread(&[]), None);
    }

    #[test]
    fn the_spread_is_the_smallest_and_largest_value() {
        assert_eq!(spread(&[4.1, 4.0, 9.0, 4.2]), Some((4.0, 9.0)));
        assert_eq!(spread(&[2.0]), Some((2.0, 2.0)));
    }

    #[test]
    fn a_steady_run_is_not_slow() {
        assert!(!is_slow(&[100, 200, 300, 400]));
    }

    #[test]
    fn a_stalled_second_makes_a_run_slow() {
        // The third second moved 5 against a peak of 100, which is under a tenth.
        assert!(is_slow(&[100, 200, 205, 305]));
    }

    #[test]
    fn exactly_a_tenth_of_the_peak_is_not_slow() {
        assert!(!is_slow(&[100, 110, 210]));
        assert!(is_slow(&[100, 109, 209]));
    }

    #[test]
    fn a_collapse_to_nothing_is_slow() {
        assert!(is_slow(&[100, 200, 200, 200]));
    }

    #[test]
    fn short_and_empty_runs_are_not_slow() {
        assert!(!is_slow(&[]));
        assert!(!is_slow(&[500]));
        assert!(!is_slow(&[0, 0, 0]));
    }

    #[test]
    fn percent_of_a_ceiling() {
        assert_eq!(percent_of(8.0, 10.0), Some(80.0));
        assert_eq!(percent_of(8.0, 0.0), None);
    }

    #[test]
    fn the_dip_is_the_lowest_second_over_the_median_second() {
        // Seconds: 50 (ramp up, left out), 100, 40, 100, 100, 10 (cut short, left out).
        let progress = [50, 150, 190, 290, 390, 400];
        assert_eq!(dip(&progress), Some(0.4));
    }

    #[test]
    fn the_first_and_last_second_do_not_count_toward_the_dip() {
        // A slow first second and a slow last second leave a flat run with no dip at all.
        assert_eq!(dip(&[1, 101, 201, 301, 302]), Some(1.0));
    }

    #[test]
    fn a_collapse_in_the_middle_is_a_dip_of_nearly_zero() {
        let progress = [100, 200, 300, 301, 400, 500, 600];
        let d = dip(&progress).expect("a dip");
        assert!((d - 0.01).abs() < 1e-9, "{d}");
    }

    #[test]
    fn a_run_too_short_for_a_middle_has_no_dip() {
        assert_eq!(dip(&[]), None);
        assert_eq!(dip(&[100, 200]), None);
        assert_eq!(dip(&[100, 200, 300]), Some(1.0));
    }

    #[test]
    fn a_median_second_of_nothing_has_no_ratio() {
        assert_eq!(dip(&[0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn the_worst_dip_is_the_lowest_across_runs() {
        let steady: &[u64] = &[100, 200, 300, 400];
        let dipping: &[u64] = &[100, 200, 220, 320, 420];
        let short: &[u64] = &[100];
        assert_eq!(worst_dip([steady, dipping, short]), Some(0.2));
        assert_eq!(worst_dip([short]), None);
    }

    #[test]
    fn a_run_is_an_outlier_above_one_and_a_half_times_the_fastest() {
        // 2.0 Gbit/s takes 3 times as long as 6.0, 4.0 takes exactly 1.5 times, 4.1 less.
        assert_eq!(outliers(&[6.0, 6.0, 2.0, 4.0, 4.1]), 1);
        assert_eq!(outliers(&[6.0, 3.9]), 1);
        assert_eq!(outliers(&[]), 0);
        assert_eq!(outliers(&[5.0]), 0);
    }
}
