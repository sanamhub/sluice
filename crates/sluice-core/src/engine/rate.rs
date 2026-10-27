//! One byte rate shared by the transfers of a command. The bytes are paced against a single
//! schedule, so two transfers of one command share the cap instead of each taking it.

use std::{sync::Mutex, time::Duration};

use crate::Error;

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

    /// The cap in `text`, such as `50M` or `2MiB`. `K`, `M` and `G` are 1024, and a following `iB`
    /// or `B` is accepted, as is no suffix for bytes.
    pub fn parse(text: &str) -> Result<std::sync::Arc<Self>, Error> {
        let text = text.trim();
        let (number, unit) = text.split_at(
            text.find(|c: char| !c.is_ascii_digit())
                .unwrap_or(text.len()),
        );
        let number: u64 = number.parse().map_err(|_| {
            Error::Policy(format!(
                "{text} is not a rate; write bytes such as 500K or 50M"
            ))
        })?;
        let unit = unit.trim().to_ascii_lowercase();
        let factor = match unit.as_str() {
            "" | "b" | "ib" => 1,
            "k" | "kb" | "kib" => 1024,
            "m" | "mb" | "mib" => 1024 * 1024,
            "g" | "gb" | "gib" => 1024 * 1024 * 1024,
            _ => {
                return Err(Error::Policy(format!(
                    "{text} is not a rate; write bytes such as 500K or 50M"
                )));
            }
        };
        let bytes = number
            .checked_mul(factor)
            .ok_or_else(|| Error::Policy(format!("{text} is larger than this machine can hold")))?;
        Self::new(bytes)
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
            let start = (*next).max(now);
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
        assert_eq!(Rate::parse("1234").unwrap().bytes_per_second(), 1234.0);
        assert!(Rate::parse("fast").is_err());
        assert!(Rate::parse("0").is_err());
    }
}
