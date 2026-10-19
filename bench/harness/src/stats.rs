//! The arithmetic behind `summary.md`: median, spread, slow runs and percent of a ceiling.

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

/// True if any whole second moved less than a tenth of what the run's busiest second moved.
/// `progress` is the cumulative payload at the end of each second, as `fetch` prints it.
pub fn is_slow(progress: &[u64]) -> bool {
    let mut previous = 0;
    let seconds: Vec<u64> = progress
        .iter()
        .map(|&total| {
            let moved = total.saturating_sub(previous);
            previous = total;
            moved
        })
        .collect();
    let Some(&peak) = seconds.iter().max() else {
        return false;
    };
    seconds
        .iter()
        .any(|&moved| (moved as f64) < 0.1 * peak as f64)
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
}
