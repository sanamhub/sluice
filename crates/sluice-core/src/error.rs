/// The only error type in Sluice. Six variants, so callers can decide on retry and exit codes
/// without matching on message text.
///
/// # Examples
///
/// ```
/// use sluice_core::Error;
///
/// let err = Error::Verify("range 0..4096 does not match the root".to_string());
/// assert!(matches!(err, Error::Verify(_)));
/// assert_eq!(err.to_string(), "verification failed: range 0..4096 does not match the root");
/// ```
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A peer or relay could not be reached, or the connection failed mid-transfer.
    #[error("network error: {0}")]
    Network(String),

    /// Received bytes did not match the BLAKE3 content root. The transfer fails and the bytes are
    /// never returned to the caller.
    #[error("verification failed: {0}")]
    Verify(String),

    /// The store or the filesystem failed, for example a full disk, a permission error or a
    /// quota error. A resumable transfer is left behind, not a panic.
    #[error("storage error: {0}")]
    Storage(String),

    /// A configured policy refused the operation.
    #[error("policy denied: {0}")]
    Policy(String),

    /// The caller cancelled the operation.
    #[error("cancelled: {0}")]
    Cancelled(String),

    /// An internal invariant was broken. This is a defect in Sluice, not a fault in the
    /// environment.
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
            (Error::Cancelled("x".into()), "cancelled: x"),
            (Error::Bug("x".into()), "internal bug: x"),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }
}
