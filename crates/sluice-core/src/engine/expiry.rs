//! How long a ticket stays good: `24h` by default, `--expires 1h`, `7d` or `never`.

use crate::Error;

/// The default lifetime of a ticket.
pub const DEFAULT_SECONDS: u64 = 24 * 60 * 60;

/// A duration in seconds from now, or no expiry. The text is one number and one unit in
/// `s`, `m`, `h`, `d` or `w`, or the word `never`. Spaces and signs are refused.
pub fn parse(text: &str) -> Result<Option<u64>, Error> {
    if text == "never" {
        return Ok(None);
    }
    let Some((number, unit)) = text.split_at_checked(text.len().saturating_sub(1)) else {
        return Err(refuse(text));
    };
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 60 * 60,
        "d" => 24 * 60 * 60,
        "w" => 7 * 24 * 60 * 60,
        _ => return Err(refuse(text)),
    };
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return Err(refuse(text));
    }
    let value: u64 = number.parse().map_err(|_| refuse(text))?;
    value
        .checked_mul(factor)
        .filter(|seconds| *seconds > 0)
        .map(Some)
        .ok_or_else(|| refuse(text))
}

/// The absolute UTC millisecond at which `seconds` from `now_ms` ends, without wrapping.
pub fn deadline(now_ms: u64, seconds: Option<u64>) -> Option<u64> {
    seconds.map(|seconds| now_ms.saturating_add(seconds.saturating_mul(1000)))
}

fn refuse(text: &str) -> Error {
    Error::Policy(format!(
        "{text:?} is not a lifetime; write a positive number and s, m, h, d or w, or never"
    ))
}

#[cfg(test)]
mod tests {
    use super::{deadline, parse};

    #[test]
    fn the_three_forms_read() {
        assert_eq!(parse("1h").unwrap(), Some(3600));
        assert_eq!(parse("7d").unwrap(), Some(7 * 86_400));
        assert_eq!(parse("30m").unwrap(), Some(1800));
        assert_eq!(parse("never").unwrap(), None);
    }

    #[test]
    fn zero_spaces_signs_and_other_units_are_refused() {
        for bad in ["", "0h", " 1h", "1 h", "-1h", "1H", "1", "h", "1.5h"] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn a_deadline_adds_milliseconds_without_wrapping() {
        assert_eq!(deadline(1_000, Some(2)), Some(3_000));
        assert_eq!(deadline(1_000, None), None);
        assert_eq!(deadline(u64::MAX, Some(1)), Some(u64::MAX));
    }
}
