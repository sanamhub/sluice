//! What a failure is called, and the exit code it gives.

use sluice_core::Error;

/// The exit code for an error, from the variant alone, as the table under Errors in
/// `docs/design.md` gives them. Every command goes through this one function, so a script can rely
/// on a code meaning the same thing whatever it ran. Exit code 2 is not here: clap gives it to a
/// bad command line.
pub fn exit_code(error: &Error) -> u8 {
    match error {
        Error::Network(_) => 3,
        Error::Verify(_) => 4,
        Error::Storage(_) => 5,
        Error::Policy(_) => 6,
        Error::Cancelled => 130,
        Error::Bug(_) => 70,
    }
}

/// The variant in lower case, which is the `kind` of the JSON error.
pub fn kind(error: &Error) -> &'static str {
    match error {
        Error::Network(_) => "network",
        Error::Verify(_) => "verify",
        Error::Storage(_) => "storage",
        Error::Policy(_) => "policy",
        Error::Cancelled => "cancelled",
        Error::Bug(_) => "bug",
    }
}

/// The message without the variant's name in front of it, which `kind` already says.
pub fn message(error: &Error) -> &str {
    match error {
        Error::Network(text)
        | Error::Verify(text)
        | Error::Storage(text)
        | Error::Policy(text)
        | Error::Bug(text) => text,
        Error::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, exit_code, kind, message};

    #[test]
    fn each_variant_has_the_code_the_design_gives_it() {
        let cases = [
            (Error::Network(String::new()), 3),
            (Error::Verify(String::new()), 4),
            (Error::Storage(String::new()), 5),
            (Error::Policy(String::new()), 6),
            (Error::Cancelled, 130),
            (Error::Bug(String::new()), 70),
        ];
        for (error, code) in cases {
            assert_eq!(exit_code(&error), code, "{error}");
        }
    }

    #[test]
    fn the_kind_is_the_variant_in_lower_case_and_the_message_is_what_it_carries() {
        let cases = [
            (Error::Network("a".into()), "network", "a"),
            (Error::Verify("b".into()), "verify", "b"),
            (Error::Storage("c".into()), "storage", "c"),
            (Error::Policy("d".into()), "policy", "d"),
            (Error::Cancelled, "cancelled", "cancelled"),
            (Error::Bug("e".into()), "bug", "e"),
        ];
        for (error, name, text) in cases {
            assert_eq!(kind(&error), name);
            assert_eq!(message(&error), text);
        }
    }
}
