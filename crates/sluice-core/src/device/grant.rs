//! What a paired device may do: `deliver` in 1.1, and `control` defined for 1.2. `SPEC.md` section
//! 11.1.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// What a paired device may do, as `SPEC.md` section 11.1 defines it. `control` includes what
/// `deliver` allows.
///
/// # Examples
///
/// ```
/// use sluice_core::device::Grant;
///
/// assert!(Grant::Control.includes(Grant::Deliver));
/// assert!(!Grant::Deliver.includes(Grant::Control));
/// assert_eq!(Grant::from_wire(0), Some(Grant::Deliver));
/// assert_eq!(Grant::from_wire(2), None);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Grant {
    /// Hand the daemon a ticket and have it received into the inbox. The default.
    Deliver,
    /// Every call of the control interface, which includes what `deliver` allows.
    Control,
}

impl Grant {
    /// The byte a pairing ticket carries for this grant. `SPEC.md` section 4 gives the values.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::Grant;
    ///
    /// assert_eq!(Grant::Deliver.wire(), 0);
    /// assert_eq!(Grant::Control.wire(), 1);
    /// ```
    pub fn wire(self) -> u8 {
        match self {
            Self::Deliver => 0,
            Self::Control => 1,
        }
    }

    /// The grant a pairing ticket's byte names, or `None` for a byte no grant uses.
    pub fn from_wire(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Deliver),
            1 => Some(Self::Control),
            _ => None,
        }
    }

    /// Whether this grant allows what `wanted` allows.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::device::Grant;
    ///
    /// assert!(Grant::Deliver.includes(Grant::Deliver));
    /// assert!(Grant::Control.includes(Grant::Deliver));
    /// assert!(!Grant::Deliver.includes(Grant::Control));
    /// ```
    pub fn includes(self, wanted: Self) -> bool {
        matches!(
            (self, wanted),
            (Self::Control, _) | (Self::Deliver, Self::Deliver)
        )
    }

    /// The word a list and a pairing command print.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deliver => "deliver",
            Self::Control => "control",
        }
    }
}

impl fmt::Display for Grant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Grant {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "deliver" => Ok(Self::Deliver),
            "control" => Ok(Self::Control),
            other => Err(format!(
                "{other} is not a grant; the grants are deliver and control"
            )),
        }
    }
}
