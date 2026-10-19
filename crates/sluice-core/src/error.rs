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

#[cfg(test)]
mod tests {
    use super::Error;

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
}
