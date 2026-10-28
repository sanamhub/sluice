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
            let span = Duration::from_secs_f64(bytes as f64 / self.bytes_per_second);
            *next = start + span;
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

impl std::fmt::Debug for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rate")
            .field("bytes_per_second", &self.bytes_per_second)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::Rate;
    use std::time::Duration;

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
