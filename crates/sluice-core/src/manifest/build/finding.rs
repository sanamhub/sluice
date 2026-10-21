//! What the build lists: a name a receiver would reject, or an entry it left out, and why.

use std::fmt;
use std::io;

/// Why a name was listed. The wording is for people; a program reads the variant.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::Reason;
///
/// assert_eq!(Reason::NotUtf8.to_string(), "the name is not valid UTF-8");
/// assert_eq!(
///     Reason::PathRule(7).to_string(),
///     "a receiver rejects the name (path rule 7)"
/// );
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The name breaks path rule 1 to 8, the number given. Rule 2 is never given: the sender
    /// writes the NFC form of every name.
    PathRule(u8),
    /// The name on disk is not valid UTF-8, so it has no path in a manifest.
    NotUtf8,
    /// Another name on disk is the same path after NFC. Which one a receiver would write is
    /// not defined, so neither is sent.
    SameAfterNfc,
    /// The path is the same as another after full case folding and NFC.
    CaseCollision,
    /// A file or folder could not be opened or read.
    Unreadable(io::ErrorKind),
    /// A symlink whose target breaks link target rule 1 to 5, the number given. Skipped.
    LinkTarget(u8),
    /// A symlink whose target is not valid UTF-8. Skipped.
    LinkTargetNotUtf8,
    /// A socket, FIFO, device or other entry that is neither file, folder nor symlink. Skipped.
    NotAFileOrFolder,
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PathRule(rule) => write!(f, "a receiver rejects the name (path rule {rule})"),
            Self::NotUtf8 => f.write_str("the name is not valid UTF-8"),
            Self::SameAfterNfc => f.write_str("another name on disk is the same path after NFC"),
            Self::CaseCollision => f.write_str("the same path as another after case folding"),
            Self::Unreadable(kind) => write!(f, "cannot be read ({kind})"),
            Self::LinkTarget(rule) => {
                write!(f, "the link target breaks link target rule {rule}")
            }
            Self::LinkTargetNotUtf8 => f.write_str("the link target is not valid UTF-8"),
            Self::NotAFileOrFolder => f.write_str("is not a file, a folder or a symlink"),
        }
    }
}

/// One name the build listed, and why. `path` is the path as the sender sees it, relative to the
/// folder that was sent, with `/` between components and the bytes that are not UTF-8 replaced
/// by U+FFFD. It is not escaped: a name can hold a newline or a terminal escape, and whoever
/// prints it has to deal with that.
///
/// # Examples
///
/// ```
/// use sluice_core::manifest::{Finding, Reason};
///
/// let finding = Finding { path: "a/con".to_string(), reason: Reason::PathRule(8) };
/// assert_eq!(finding.reason, Reason::PathRule(8));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The path as the sender sees it.
    pub path: String,
    /// Why it was listed.
    pub reason: Reason,
}
