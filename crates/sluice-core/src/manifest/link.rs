//! The target of a symlink entry, checked against the link target rules in `SPEC.md` section 8.
//!
//! The rules read the text and nothing on disk. A target that passes them can still reach another
//! symlink that is already in the output folder, which is the writer's concern, as for paths.

use super::path::{MAX_PATH_BYTES, ManifestPath, component_rule};
use crate::Error;

/// A symlink target that passed link target rules 1 to 4: relative, in NFC, with forward
/// slashes, and no component that is not a name or `..`.
///
/// Rule 5 needs the folder that holds the link, so [`LinkTarget::stays_inside`] asks it. A
/// [`Manifest`](super::Manifest) never holds a target that fails it.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{link::LinkTarget, path::ManifestPath};
///
/// let target = LinkTarget::parse("../shared/data")?;
/// assert_eq!(target.as_str(), "../shared/data");
///
/// // `a/link` reaches `shared/data` beside `a`. A link at the top has no folder above it.
/// assert!(target.stays_inside(&ManifestPath::parse("a/link")?));
/// assert!(!target.stays_inside(&ManifestPath::parse("link")?));
///
/// assert!(LinkTarget::parse("/etc/passwd").is_err());
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkTarget(String);

impl LinkTarget {
    /// Applies link target rules 1 to 4 and keeps the target if it passes them all.
    ///
    /// The error is [`Error::Policy`]. Its message names the lowest rule the text breaks and
    /// never quotes the text, because a target comes from a stranger and errors reach logs.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, manifest::link::LinkTarget};
    ///
    /// assert!(LinkTarget::parse("a/../b").is_ok());
    ///
    /// let err = LinkTarget::parse("C:\\x").unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// assert_eq!(
    ///     err.to_string(),
    ///     "policy denied: manifest link target rule 4: the target has a component that breaks \
    ///      path rule 2, 5, 6, 7 or 8"
    /// );
    /// ```
    pub fn parse(text: &str) -> Result<Self, Error> {
        Self::parse_rule(text).map_err(|rule| {
            Error::Policy(format!(
                "manifest link target rule {rule}: the target {}",
                describe(rule)
            ))
        })
    }

    /// [`LinkTarget::parse`] for callers that want the rule number and not a message.
    pub(super) fn parse_rule(text: &str) -> Result<Self, u8> {
        // Before anything walks the text, so a megabyte of hostile input costs one comparison.
        if text.is_empty() || text.len() > MAX_PATH_BYTES {
            return Err(1);
        }
        if text.starts_with('/') || text.ends_with('/') {
            return Err(2);
        }
        if text.split('/').any(|c| matches!(c, "" | ".")) {
            return Err(3);
        }
        // `..` is a name to path rule 4 and an allowed step here, so it is left out of the check.
        if text
            .split('/')
            .any(|c| c != ".." && component_rule(c).is_some())
        {
            return Err(4);
        }
        Ok(Self(text.to_owned()))
    }

    /// The target as it was parsed, with `/` between components.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::link::LinkTarget;
    ///
    /// assert_eq!(LinkTarget::parse("日本/語")?.as_str(), "日本/語");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rule 5: resolved from the folder that holds `link`, one component at a time, no `..` steps
    /// above the output folder.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::{link::LinkTarget, path::ManifestPath};
    ///
    /// let link = ManifestPath::parse("a/b/link")?;
    /// assert!(LinkTarget::parse("../../x")?.stays_inside(&link));
    /// assert!(!LinkTarget::parse("../../../x")?.stays_inside(&link));
    /// // Going down first and back up is fine. The text is all that is looked at.
    /// assert!(LinkTarget::parse("c/../../x")?.stays_inside(&link));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn stays_inside(&self, link: &ManifestPath) -> bool {
        // The folder that holds the link is the path without its last component.
        let mut depth = link.components().count() - 1;
        for component in self.0.split('/') {
            if component == ".." {
                let Some(above) = depth.checked_sub(1) else {
                    return false;
                };
                depth = above;
            } else {
                depth += 1;
            }
        }
        true
    }
}

fn describe(rule: u8) -> &'static str {
    match rule {
        1 => "is empty or longer than 4096 bytes",
        2 => "begins or ends with `/`",
        3 => "has an empty or `.` component",
        _ => "has a component that breaks path rule 2, 5, 6, 7 or 8",
    }
}

#[cfg(test)]
mod tests;
