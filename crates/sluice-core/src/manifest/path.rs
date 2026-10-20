//! The path of one manifest entry, checked against the receiver rules in `SPEC.md` section 8.
//!
//! The rules look only at the text of the path, the same on every platform. A receiver that
//! checked only the rules of the OS it runs on would accept a tree on Linux that it rejects on
//! Windows, and the two would write different folders from one manifest.

use unicode_normalization::is_nfc;

use crate::Error;

/// The longest path, in UTF-8 bytes (rule 1).
const MAX_PATH_BYTES: usize = 4096;
/// The longest component, in UTF-8 bytes (rule 5). It is the NTFS and ext4 limit.
const MAX_COMPONENT_BYTES: usize = 255;
/// How many characters of an offending component an error quotes. A hostile path is up to
/// 4096 bytes and arrives from a stranger, so the error must not copy it into a log.
const QUOTED_CHARS: usize = 32;

/// A manifest path that passed every receiver rule: relative, in NFC, with forward slashes, and
/// with no component that Windows, macOS or Linux would read as something other than a file name.
///
/// It holds the text only. Joining it onto a directory is the writer's job, because a symlink
/// that is already on disk is outside what the text can say.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::path::ManifestPath;
///
/// let path = ManifestPath::parse("photos/2026/IMG_0001.jpg")?;
/// assert_eq!(path.as_str(), "photos/2026/IMG_0001.jpg");
/// assert_eq!(path.components().collect::<Vec<_>>(), ["photos", "2026", "IMG_0001.jpg"]);
///
/// assert!(ManifestPath::parse("../etc/passwd").is_err());
/// # Ok::<(), sluice_core::Error>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ManifestPath(String);

impl ManifestPath {
    /// Applies the eight rules of `SPEC.md` section 8 and keeps the path if it passes them all.
    ///
    /// The path is never normalized: text that is not in NFC is rejected, because two spellings
    /// of one name would name one file. The error is [`Error::Policy`]. Its message names the
    /// lowest rule the path breaks and quotes at most 32 characters of the offending component,
    /// so it is safe to log whatever the sender wrote.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, manifest::path::ManifestPath};
    ///
    /// assert!(ManifestPath::parse("a/.hidden/..a").is_ok());
    ///
    /// let err = ManifestPath::parse("a/../b").unwrap_err();
    /// assert!(matches!(err, Error::Policy(_)));
    /// assert_eq!(
    ///     err.to_string(),
    ///     r#"policy denied: manifest path rule 4: component ".." is not a name"#
    /// );
    /// ```
    pub fn parse(path: &str) -> Result<Self, Error> {
        match check(path) {
            Ok(()) => Ok(Self(path.to_owned())),
            Err(violation) => Err(Error::Policy(violation.message())),
        }
    }

    /// The path as it was parsed, with `/` between components.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::path::ManifestPath;
    ///
    /// assert_eq!(ManifestPath::parse("é/日本語.txt")?.as_str(), "é/日本語.txt");
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The components in order. None is empty, and none is `.` or `..`.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::manifest::path::ManifestPath;
    ///
    /// let path = ManifestPath::parse("a/b/c")?;
    /// assert_eq!(path.components().last(), Some("c"));
    /// # Ok::<(), sluice_core::Error>(())
    /// ```
    pub fn components(&self) -> std::str::Split<'_, char> {
        self.0.split('/')
    }
}

/// A rule a path breaks. Kept apart from [`Error`] so the tests and the vectors can compare the
/// rule number without reading a message that is for people.
#[derive(Debug)]
struct Violation<'a> {
    rule: u8,
    reason: String,
    component: Option<&'a str>,
}

impl<'a> Violation<'a> {
    fn path(rule: u8, reason: &str) -> Self {
        Self {
            rule,
            reason: reason.to_owned(),
            component: None,
        }
    }

    fn component(rule: u8, reason: impl Into<String>, component: &'a str) -> Self {
        Self {
            rule,
            reason: reason.into(),
            component: Some(component),
        }
    }

    fn message(&self) -> String {
        let rule = self.rule;
        match self.component {
            Some(component) => {
                format!(
                    "manifest path rule {rule}: component {} {}",
                    quote(component),
                    self.reason
                )
            }
            None => format!("manifest path rule {rule}: the path {}", self.reason),
        }
    }
}

/// The first 32 characters of a component, in quotes with control characters escaped, and a
/// trailing `...` when it was cut.
fn quote(component: &str) -> String {
    let shown: String = component.chars().take(QUOTED_CHARS).collect();
    let cut = component.chars().nth(QUOTED_CHARS).is_some();
    format!("{shown:?}{}", if cut { "..." } else { "" })
}

/// The lowest-numbered rule `path` breaks, or `Ok` if it breaks none.
///
/// The vectors say that a path with two faults names the lowest rule, so every rule is looked at
/// and the minimum is kept, instead of returning at the first fault found while reading left to
/// right.
fn check(path: &str) -> Result<(), Violation<'_>> {
    if path.is_empty() {
        return Err(Violation::path(1, "is empty"));
    }
    // Before anything walks the text, so a megabyte of hostile input costs one comparison.
    if path.len() > MAX_PATH_BYTES {
        return Err(Violation::path(1, "is longer than 4096 bytes"));
    }
    let edge = if path.starts_with('/') {
        Some(Violation::path(3, "begins with `/`"))
    } else if path.ends_with('/') {
        Some(Violation::path(3, "ends with `/`"))
    } else {
        None
    };
    // Rule 2 can only be broken inside a component: `/` is its own starter and never combines.
    let component = path
        .split('/')
        .filter_map(component_violation)
        .min_by_key(|v| v.rule);
    match edge.into_iter().chain(component).min_by_key(|v| v.rule) {
        Some(violation) => Err(violation),
        None => Ok(()),
    }
}

/// The lowest rule among 2 and 4 to 8 that one component breaks.
fn component_violation(component: &str) -> Option<Violation<'_>> {
    let broken = |rule, reason: &str| Some(Violation::component(rule, reason, component));
    if !is_nfc(component) {
        return broken(2, "is not in NFC");
    }
    if matches!(component, "" | "." | "..") {
        let reason = if component.is_empty() {
            "is empty"
        } else {
            "is not a name"
        };
        return broken(4, reason);
    }
    if component.len() > MAX_COMPONENT_BYTES {
        return broken(5, "is longer than 255 bytes");
    }
    if let Some(bad) = component.chars().find(|&c| is_forbidden(c)) {
        return Some(Violation::component(
            6,
            format!("contains {bad:?}"),
            component,
        ));
    }
    if component.ends_with(['.', ' ']) {
        return broken(7, "ends with `.` or a space");
    }
    if is_reserved_device_name(component) {
        return broken(8, "is a Windows reserved device name");
    }
    None
}

/// Rule 6: the characters Windows refuses in a name, the two that mean something on some
/// platform (`\` and `:`), and the control characters.
fn is_forbidden(c: char) -> bool {
    matches!(
        c,
        '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*' | '\u{0}'..='\u{1f}' | '\u{7f}'
    )
}

/// Rule 8. Windows reads `con.tar.gz` and `CON .txt` as the console, so the name is what comes
/// before the first `.`, without trailing spaces. U+0131 counts as `I`: it is not clear that a
/// Windows name lookup keeps a dotless `CONIN$` apart from `CONIN$`, so the rule rejects both.
fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ');
    // The longest name is `CONOUT$`, 7 bytes. Skipping the rest also keeps the fold below short.
    if stem.len() > 7 {
        return false;
    }
    let folded: String = stem
        .chars()
        .map(|c| {
            if c == '\u{131}' {
                'i'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect();
    if matches!(
        folded.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) {
        return true;
    }
    let numbered = folded
        .strip_prefix("com")
        .or_else(|| folded.strip_prefix("lpt"));
    numbered.is_some_and(|n| {
        let mut chars = n.chars();
        matches!(chars.next(), Some('0'..='9' | '¹' | '²' | '³')) && chars.next().is_none()
    })
}

#[cfg(test)]
mod tests;
