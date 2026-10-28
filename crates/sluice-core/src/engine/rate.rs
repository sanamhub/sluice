//! One byte rate shared by the transfers of a command. The bytes are paced against a single
//! schedule, so two transfers of one command share the cap instead of each taking it.

use std::{sync::Mutex, time::Duration};

use crate::Error;

/// How far behind its schedule a wait may wake before the bytes are paid back at once. A timer fires
/// on a tick, so a wait of a fraction of a tick still takes a whole one; without the credit a 64M cap
/// on small chunks delivers only the chunk per tick, and the measured rate was a tenth of the cap on
/// Windows and a quarter on a virtual disk. Twenty milliseconds is a few ticks, which absorbs the lateness
/// without letting a transfer spring past its cap by more than a moment's worth of bytes.
const BURST: Duration = Duration::from_millis(20);

/// The longest a single wait is scheduled for. A caller's chunk is bounded, so a wait this long means
/// the cap and the chunk are both absurd; capping keeps the arithmetic total instead of letting a
/// huge count overflow the schedule and panic.
const LONGEST_WAIT: Duration = Duration::from_secs(365 * 24 * 60 * 60);
/// A cap in bytes per second. Every caller reserves its bytes on one schedule and waits for its
/// turn, so the total over any long span is the cap and no transfer is starved.
pub struct Rate {
    bytes_per_second: f64,
    next: Mutex<tokio::time::Instant>,
}

impl Rate {
    /// A cap of `bytes_per_second`. Zero is refused: it would stop the transfer for ever.
    pub fn new(bytes_per_second: u64) -> Result<std::sync::Arc<Self>, Error> {
        if bytes_per_second == 0 {
            return Err(Error::Policy(
                "a rate of zero bytes per second would never finish".to_string(),
            ));
        }
        Ok(std::sync::Arc::new(Self {
            bytes_per_second: bytes_per_second as f64,
            next: Mutex::new(tokio::time::Instant::now()),
        }))
    }

    /// The cap in `text`, such as `50M`, `50MB` or `2MiB`, read by the one size parser every size
    /// flag uses: `K`, `M`, `G` and `T` alone or with `iB` are powers of 1024, the same letters
    /// with `B` are powers of 1000, and a bare number is bytes.
    pub fn parse(text: &str) -> Result<std::sync::Arc<Self>, Error> {
        Self::new(crate::range::ByteSize::parse(text)?.bytes())
    }
    /// Waits until `bytes` more fit under the cap.
    pub async fn acquire(&self, bytes: u64) {
        if bytes == 0 {
            return;
        }
        let now = tokio::time::Instant::now();
        let start = {
            let mut next = self
                .next
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // A wait ends on the timer's tick, not at the instant it was asked for, so it can be
            // late by a tick and the schedule falls behind by that much every time. The credit lets
            // the next calls go out at once until the schedule is back where the clock is, and no
            // further than that: after a long pause the schedule moves to one burst before now, so
            // a transfer that was idle cannot empty a burst's worth of bytes at full speed.
            let earliest = now.checked_sub(BURST).unwrap_or(now);
            let start = (*next).max(earliest);
            let span = span_of(bytes, self.bytes_per_second);
            *next = start.checked_add(span).unwrap_or(start + LONGEST_WAIT);
            start
        };
        if start > now {
            tokio::time::sleep_until(start).await;
        }
    }

    /// The cap, for a test that checks how the text was read.
    #[cfg(test)]
    fn bytes_per_second(&self) -> f64 {
        self.bytes_per_second
    }
}

/// How long `bytes` take at `bytes_per_second`. The division is a float, so a count near `u64::MAX`
/// is a number of seconds no clock can hold; the wait is capped instead of panicking, which is what
/// a cap of a few bytes per second and a chunk of `u64::MAX` would otherwise do.
fn span_of(bytes: u64, bytes_per_second: f64) -> Duration {
    let seconds = bytes as f64 / bytes_per_second;
    Duration::try_from_secs_f64(seconds)
        .map(|span| span.min(LONGEST_WAIT))
        .unwrap_or(LONGEST_WAIT)
}

impl std::fmt::Debug for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rate")
            .field("bytes_per_second", &self.bytes_per_second)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{LONGEST_WAIT, Rate, span_of};
    use std::time::Duration;

    /// A count that no clock can hold is capped, not a panic. The schedule is a float division, so
    /// `u64::MAX` bytes at one byte a second is far past what a `Duration` or an `Instant` holds.
    #[test]
    fn a_wait_that_no_clock_can_hold_is_capped() {
        assert_eq!(span_of(u64::MAX, 1.0), LONGEST_WAIT);
        assert_eq!(span_of(u64::MAX, 1e-9), LONGEST_WAIT);
        assert_eq!(span_of(64, 64.0), Duration::from_secs(1));
        assert_eq!(span_of(0, 64.0), Duration::ZERO);
    }

    /// The limiter itself is total for that count: the second call schedules its wait a year out
    /// instead of panicking on arithmetic that overflows, and a timer that never comes does not
    /// panic the schedule either.
    #[tokio::test(start_paused = true)]
    async fn acquiring_the_largest_count_does_not_panic() {
        let rate = Rate::new(1).unwrap();
        // The first call spends the schedule it was given; the second one is a year behind.
        rate.acquire(u64::MAX).await;
        let short = tokio::time::timeout(Duration::from_millis(1), rate.acquire(u64::MAX)).await;
        assert!(short.is_err(), "a wait of years finished at once");
    }

    #[test]
    fn a_rate_reads_with_a_binary_suffix_or_none() {
        assert_eq!(
            Rate::parse("50M").unwrap().bytes_per_second(),
            50.0 * 1024.0 * 1024.0
        );
        assert_eq!(
            Rate::parse("500K").unwrap().bytes_per_second(),
            500.0 * 1024.0
        );
        assert_eq!(
            Rate::parse("2MiB").unwrap().bytes_per_second(),
            2.0 * 1024.0 * 1024.0
        );
        // The one parser every size flag uses: with `B` the letter is a power of 1000.
        assert_eq!(Rate::parse("2MB").unwrap().bytes_per_second(), 2_000_000.0);
        assert_eq!(
            Rate::parse("1GiB").unwrap().bytes_per_second(),
            1_073_741_824.0
        );
        assert_eq!(Rate::parse("1234").unwrap().bytes_per_second(), 1234.0);
        assert!(Rate::parse("fast").is_err());
        assert!(Rate::parse("0").is_err());
    }

    /// A real timer wakes on a tick, not at the instant it was asked for. A receiver reads the
    /// store in 16 KiB steps, so at 64 MiB/s each step is a quarter of a tick: if every step waits a
    /// whole tick, ten seconds deliver only the bytes of ten thousand ticks. With the credit the
    /// schedule catches up after each late wake, and the cap holds to within five percent.
    #[tokio::test(start_paused = true)]
    async fn a_cap_is_met_when_a_wait_wakes_on_a_tick() {
        const CHUNK: u64 = 16 * 1024;
        const CAP: u64 = 64 << 20;
        let rate = Rate::new(CAP).unwrap();
        let started = tokio::time::Instant::now();
        let mut sent = 0u64;
        while started.elapsed() < Duration::from_secs(10) {
            let before = tokio::time::Instant::now();
            rate.acquire(CHUNK).await;
            sent += CHUNK;
            let now = tokio::time::Instant::now();
            if now > before {
                let into_tick = now.duration_since(started).subsec_nanos() % 1_000_000;
                if into_tick != 0 {
                    tokio::time::advance(Duration::from_nanos(1_000_000 - u64::from(into_tick)))
                        .await;
                }
            }
        }
        let achieved = sent as f64 / started.elapsed().as_secs_f64();
        let cap = CAP as f64;
        assert!(
            (0.95 * cap..=1.05 * cap).contains(&achieved),
            "{achieved} bytes per second against a cap of {cap}"
        );
    }
}
