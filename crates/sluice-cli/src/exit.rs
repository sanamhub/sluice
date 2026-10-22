//! What a failure is called, and the exit code it gives.

use sluice_core::{Error, ErrorKind};

/// Why a command did not finish.
pub enum Failure {
    /// The command returned an error.
    Error(Error),
    /// The command panicked. Carries the panic message.
    Panic(String),
}

/// The exit code for a failure, from the variant alone, as the table under Errors in
/// `docs/design.md` gives them. Every command goes through this one function, so a script can rely
/// on a code meaning the same thing whatever it ran. A panic is a bug in sluice and gives the code
/// of `Bug`, not the 101 that Rust gives. Exit code 2 is not here: clap gives it to a bad command
/// line.
pub fn exit_code(failure: &Failure) -> u8 {
    match failure {
        Failure::Error(error) => code_of(error.kind()),
        Failure::Panic(_) => code_of(ErrorKind::Bug),
    }
}

/// The exit code of a kind of error. `sluice doctor` uses it for the kind of a failed check.
pub fn code_of(kind: ErrorKind) -> u8 {
    match kind {
        ErrorKind::Network => 3,
        ErrorKind::Verify => 4,
        ErrorKind::Storage => 5,
        ErrorKind::Policy => 6,
        ErrorKind::Cancelled => 130,
        ErrorKind::Bug => 70,
    }
}

/// The variant in lower case, which is the `kind` of the JSON error.
pub fn kind(failure: &Failure) -> &'static str {
    match failure {
        Failure::Error(Error::Network(_)) => "network",
        Failure::Error(Error::Verify(_)) => "verify",
        Failure::Error(Error::Storage(_)) => "storage",
        Failure::Error(Error::Policy(_)) => "policy",
        Failure::Error(Error::Cancelled) => "cancelled",
        Failure::Error(Error::Bug(_)) | Failure::Panic(_) => "bug",
    }
}

/// The message without the variant's name in front of it, which `kind` already says.
pub fn message(failure: &Failure) -> String {
    match failure {
        Failure::Error(
            Error::Network(text)
            | Error::Verify(text)
            | Error::Storage(text)
            | Error::Policy(text)
            | Error::Bug(text),
        ) => text.clone(),
        Failure::Error(Error::Cancelled) => "cancelled".to_string(),
        Failure::Panic(text) => format!("internal error: {text}"),
    }
}

/// The line a person reads after `sluice: `. For an error it is the text `Error` displays, which
/// starts with the variant. A panic reads `internal error: <panic message>`.
pub fn text(failure: &Failure) -> String {
    match failure {
        Failure::Error(error) => error.to_string(),
        Failure::Panic(_) => message(failure),
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, Failure, exit_code, kind, message, text};

    fn all() -> Vec<(Failure, u8, &'static str, &'static str)> {
        vec![
            (
                Failure::Error(Error::Network("a".into())),
                3,
                "network",
                "a",
            ),
            (Failure::Error(Error::Verify("b".into())), 4, "verify", "b"),
            (
                Failure::Error(Error::Storage("c".into())),
                5,
                "storage",
                "c",
            ),
            (Failure::Error(Error::Policy("d".into())), 6, "policy", "d"),
            (
                Failure::Error(Error::Cancelled),
                130,
                "cancelled",
                "cancelled",
            ),
            (Failure::Error(Error::Bug("e".into())), 70, "bug", "e"),
            (Failure::Panic("f".into()), 70, "bug", "internal error: f"),
        ]
    }

    #[test]
    fn each_variant_has_the_code_the_design_gives_it_and_a_panic_has_the_code_of_bug() {
        for (failure, code, ..) in all() {
            assert_eq!(exit_code(&failure), code, "{}", text(&failure));
        }
    }

    #[test]
    fn the_kind_is_the_variant_in_lower_case_and_the_message_is_what_it_carries() {
        for (failure, _, name, carried) in all() {
            assert_eq!(kind(&failure), name);
            assert_eq!(message(&failure), carried);
        }
    }

    #[test]
    fn the_text_of_a_panic_is_internal_error_and_its_message() {
        assert_eq!(text(&Failure::Panic("boom".into())), "internal error: boom");
        assert_eq!(
            text(&Failure::Error(Error::Policy("no".into()))),
            "policy denied: no"
        );
    }
}
