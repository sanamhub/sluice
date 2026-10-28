/// The only error type in Sluice. Callers decide on retries from the variant alone. The message
/// is for people and can change in any release.
///
/// # Examples
///
/// ```
/// use sluice_core::Error;
///
/// let err = Error::Verify("range 0..4096 does not match the root".to_string());
/// assert!(matches!(err, Error::Verify(_)));
/// assert_eq!(err.to_string(), "verification failed: range 0..4096 does not match the root");
///
/// assert_eq!(Error::Cancelled.to_string(), "cancelled");
/// ```
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A peer or relay could not be reached, or a connection failed mid-transfer. Retrying can
    /// help.
    #[error("network error: {0}")]
    Network(String),

    /// Received bytes did not match the BLAKE3 root. The bytes are discarded.
    #[error("verification failed: {0}")]
    Verify(String),

    /// The store or filesystem failed: full disk, permission or quota.
    #[error("storage error: {0}")]
    Storage(String),

    /// Sluice refused by rule: the receiver declined, a path escapes the target directory, a size
    /// limit, or a disallowed request such as an `iroh-blobs` push.
    #[error("policy denied: {0}")]
    Policy(String),

    /// The caller cancelled. Carries no message.
    #[error("cancelled")]
    Cancelled,

    /// An internal invariant broke. A defect in Sluice, never in the environment.
    #[error("internal bug: {0}")]
    Bug(String),
}

impl Error {
    /// Which of the six variants this is, without its message. A caller that keeps or compares
    /// failures, such as the transfer queue's history, keeps this and not the message, which can
    /// name a file.
    ///
    /// # Examples
    ///
    /// ```
    /// use sluice_core::{Error, ErrorKind};
    ///
    /// assert_eq!(Error::Network("reset".into()).kind(), ErrorKind::Network);
    /// assert_eq!(Error::Cancelled.kind(), ErrorKind::Cancelled);
    /// ```
    pub fn kind(&self) -> ErrorKind {
        match self {
            Error::Network(_) => ErrorKind::Network,
            Error::Verify(_) => ErrorKind::Verify,
            Error::Storage(_) => ErrorKind::Storage,
            Error::Policy(_) => ErrorKind::Policy,
            Error::Cancelled => ErrorKind::Cancelled,
            Error::Bug(_) => ErrorKind::Bug,
        }
    }
}

/// The variant of an [`Error`] on its own: the six kinds, with no message.
///
/// # Examples
///
/// ```
/// use sluice_core::ErrorKind;
///
/// let kind = ErrorKind::Storage;
/// assert_ne!(kind, ErrorKind::Policy);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// [`Error::Network`].
    Network,
    /// [`Error::Verify`].
    Verify,
    /// [`Error::Storage`].
    Storage,
    /// [`Error::Policy`].
    Policy,
    /// [`Error::Cancelled`].
    Cancelled,
    /// [`Error::Bug`].
    Bug,
}

/// The last cause below `error`, with none of the wrappers above it. A library that names its own
/// variants in `Display` would otherwise put `Error::Io` in a message a person reads, and the
/// deepest cause is the one that says what went wrong: a missing file, a full disk, a locked one.
/// An error with no cause below it is its own message, prefix and all.
///
/// # Examples
///
/// ```
/// use sluice_core::Error;
///
/// let err = Error::Storage("cannot write a file".to_string());
/// assert_eq!(sluice_core::plain(&err), "storage error: cannot write a file");
/// ```
pub fn plain(error: &(dyn std::error::Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text = cause.to_string();
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind};

    #[test]
    fn a_message_is_the_last_cause() {
        #[derive(Debug)]
        struct Outer(Inner);
        #[derive(Debug)]
        struct Inner;
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the outer call failed")
            }
        }
        impl std::fmt::Display for Inner {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the file is not there")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        impl std::error::Error for Inner {}
        assert_eq!(super::plain(&Outer(Inner)), "the file is not there");
    }

    #[test]
    fn display_is_stable() {
        let cases = [
            (Error::Network("x".into()), "network error: x"),
            (Error::Verify("x".into()), "verification failed: x"),
            (Error::Storage("x".into()), "storage error: x"),
            (Error::Policy("x".into()), "policy denied: x"),
            (Error::Cancelled, "cancelled"),
            (Error::Bug("x".into()), "internal bug: x"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn each_variant_has_its_own_kind() {
        let kinds = [
            Error::Network(String::new()).kind(),
            Error::Verify(String::new()).kind(),
            Error::Storage(String::new()).kind(),
            Error::Policy(String::new()).kind(),
            Error::Cancelled.kind(),
            Error::Bug(String::new()).kind(),
        ];
        let want = [
            ErrorKind::Network,
            ErrorKind::Verify,
            ErrorKind::Storage,
            ErrorKind::Policy,
            ErrorKind::Cancelled,
            ErrorKind::Bug,
        ];
        assert_eq!(kinds, want);
    }
}
