//! A number of bytes as a person writes it: `4G`, `4GiB`, `4GB`, `4.1G` or `512`.

use std::fmt;

use super::refuse;
use crate::Error;

/// The units a size may end with, without regard to case, and the power of 1024 or 1000 each is.
/// A unit alone or with `iB` is binary, as dd and truncate read it, and with `B` it is decimal.
const PREFIXES: [char; 5] = ['k', 'm', 'g', 't', 'p'];

/// A number of bytes.
///
/// It parses from the forms dd and truncate take, and displays in the shortest form that parses
/// back to the same number: the largest binary unit that divides it, or plain bytes.
///
/// # Examples
///
/// ```
/// use sluice_core::range::ByteSize;
///
/// assert_eq!(ByteSize::parse("4G")?.bytes(), 4 << 30);
/// assert_eq!(ByteSize::parse("4GB")?.bytes(), 4_000_000_000);
/// assert_eq!(ByteSize(4 << 30).to_string(), "4G");
/// assert_eq!(ByteSize(1000).to_string(), "1000");
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ByteSize(pub u64);

impl ByteSize {
    /// Reads a size: digits, an optional `.` and more digits, and an optional unit.
    ///
    /// - A plain number is bytes, and so is one that ends in `B`.
    /// - `K`, `M`, `G`, `T` and `P`, alone or followed by `iB`, are powers of 1024, so `4G` and
    ///   `4GiB` are both 4 × 1024³.
    /// - Followed by `B` they are powers of 1000, so `4GB` is 4 × 1000³.
    /// - Units are read without regard to case.
    /// - A decimal has digits on both sides of the point. Its exact value is rounded down to a
    ///   whole byte, so `4.1G` is 4,402,341,478 and `1.5` is 1.
    ///
    /// Fails with [`Error::Policy`], quoting the text, for an empty number, a sign, a space, a unit
    /// that is not one of these, or a value over `u64::MAX`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteSize;
    ///
    /// assert_eq!(ByteSize::parse("4.1G")?.bytes(), 4_402_341_478);
    /// assert_eq!(ByteSize::parse("2kib")?.bytes(), 2048);
    /// assert_eq!(ByteSize::parse("18446744073709551615")?.bytes(), u64::MAX);
    /// assert!(ByteSize::parse("18446744073709551616").is_err());
    /// assert!(ByteSize::parse("-1").is_err() && ByteSize::parse("4 G").is_err());
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn parse(text: &str) -> Result<Self, Error> {
        parse_in(text, text).map(ByteSize)
    }

    /// The number of bytes.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::range::ByteSize;
    ///
    /// assert_eq!(ByteSize::parse("1K")?.bytes(), 1024);
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn bytes(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ByteSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (power, prefix) in PREFIXES.iter().enumerate().rev() {
            let unit = 1u64 << (10 * (power + 1));
            if self.0 != 0 && self.0.is_multiple_of(unit) {
                return write!(f, "{}{}", self.0 / unit, prefix.to_ascii_uppercase());
            }
        }
        write!(f, "{}", self.0)
    }
}

/// The size `text` holds. `whole` is what the user wrote, which may be a whole range, and is what
/// a refusal quotes.
pub(super) fn parse_in(text: &str, whole: &str) -> Result<u64, Error> {
    if text.starts_with(['+', '-']) {
        return Err(refuse(whole, "a size has no sign"));
    }
    if text.contains(char::is_whitespace) {
        return Err(refuse(whole, "a size has no spaces"));
    }
    let digits_end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits_end);
    let unit = unit_bytes(unit).ok_or_else(|| {
        refuse(
            whole,
            "the unit is not one of B, K, M, G, T, P, each alone or with iB or B",
        )
    })?;
    let (integer, fraction) = match number.split_once('.') {
        Some((integer, fraction)) => (integer, fraction),
        None => (number, ""),
    };
    if integer.is_empty() || (number.contains('.') && fraction.is_empty()) || fraction.contains('.')
    {
        return Err(refuse(
            whole,
            "a size needs digits, and digits on both sides of a point",
        ));
    }
    scaled(integer, fraction, unit).ok_or_else(|| refuse(whole, "it is over u64::MAX bytes"))
}

/// What one of the unit is in bytes, or `None` for a unit that is not one.
fn unit_bytes(unit: &str) -> Option<u64> {
    let lower = unit.to_ascii_lowercase();
    if lower.is_empty() || lower == "b" {
        return Some(1);
    }
    let mut chars = lower.chars();
    let first = chars.next()?;
    let power = PREFIXES.iter().position(|p| *p == first)? as u32 + 1;
    match chars.as_str() {
        "" | "ib" => Some(1024u64.pow(power)),
        "b" => Some(1000u64.pow(power)),
        _ => None,
    }
}

/// `integer.fraction` times `unit`, rounded down, or `None` past `u64::MAX`. Both strings are
/// ASCII digits.
fn scaled(integer: &str, fraction: &str, unit: u64) -> Option<u64> {
    let mut whole: u128 = 0;
    for digit in integer.bytes() {
        whole = whole
            .checked_mul(10)?
            .checked_add(u128::from(digit - b'0'))?;
        if whole > u128::from(u64::MAX) {
            return None;
        }
    }
    let total = whole * u128::from(unit) + u128::from(fraction_of(fraction, unit));
    u64::try_from(total).ok()
}

/// `0.fraction` times `unit`, rounded down, exactly, for any number of digits. It works from the
/// last digit to the first, carrying the part above the point: for whole numbers `a` and `c` and
/// any `b`, `floor((a + floor(b)) / c)` is `floor((a + b) / c)`, so each step can round down and
/// the result is still exact. Every value stays under ten times `unit`.
fn fraction_of(fraction: &str, unit: u64) -> u64 {
    let mut carried: u64 = 0;
    for digit in fraction.bytes().rev() {
        carried = (u64::from(digit - b'0') * unit + carried) / 10;
    }
    carried
}
