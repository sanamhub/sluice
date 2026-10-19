//! The results directory: `raw.jsonl` as the run goes, and the other files at the end.

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;

/// A results directory with `raw.jsonl` open for appending.
pub struct Results {
    dir: PathBuf,
    raw: File,
}

impl Results {
    /// Creates `<base>/<UTC date>-<label>`. It fails if the directory exists, so a second run
    /// cannot overwrite the first.
    pub fn create(base: &Path, label: &str) -> Result<Self> {
        ensure!(
            !label.is_empty()
                && label
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
            "the label may hold letters, digits, dot, dash and underscore"
        );
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("clock is before 1970")?
            .as_secs();
        Self::create_in(base.join(format!("{}-{label}", utc_date(secs))))
    }

    /// Creates exactly `dir`, which must not exist.
    pub fn create_in(dir: PathBuf) -> Result<Self> {
        if dir.exists() {
            bail!("{} exists. Pick another label.", dir.display());
        }
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let raw = File::create(dir.join("raw.jsonl")).context("create raw.jsonl")?;
        Ok(Self { dir, raw })
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Appends one object as one line and flushes it, so an interrupted run keeps what it had.
    pub fn record(&mut self, value: &Value) -> Result<()> {
        writeln!(self.raw, "{value}").context("write raw.jsonl")?;
        self.raw.flush().context("flush raw.jsonl")
    }

    /// Writes `name` in the directory.
    pub fn write(&self, name: &str, text: &str) -> Result<()> {
        fs::write(self.dir.join(name), text).with_context(|| format!("write {name}"))
    }
}

/// Today's date in UTC.
pub fn today() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    utc_date(secs)
}

/// `YYYY-MM-DD` for seconds since the Unix epoch, in UTC.
pub fn utc_date(secs: u64) -> String {
    let (year, month, day) = civil(i64::try_from(secs / 86_400).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_match_known_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_709_164_800), "2024-02-29");
        assert_eq!(utc_date(1_781_827_200), "2026-06-19");
        // 23:59:59 and 00:00:00 UTC land on different days.
        assert_eq!(utc_date(1_760_918_399), "2025-10-19");
        assert_eq!(utc_date(1_760_918_400), "2025-10-20");
    }

    #[test]
    fn a_label_cannot_leave_the_results_directory() {
        let base = std::env::temp_dir();
        assert!(Results::create(&base, "../x").is_err());
        assert!(Results::create(&base, "").is_err());
        assert!(Results::create(&base, "a/b").is_err());
    }
}
