//! Everything a command prints goes through here. A command returns its result as a value, and
//! this module decides how it looks: text or JSON, with colour or without. That keeps the rules
//! for scripts in one place, and `README.md` states them.

use std::io::{self, Write};
use std::process::ExitCode;

use anstream::{AutoStream, ColorChoice};
use serde_json::json;
use sluice_core::Error;

use crate::colour::Colour;
use crate::exit::{Failure, exit_code, kind, message, text};

/// How results and failures are written. It follows `--json` and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

/// What a command produced.
pub enum Outcome {
    /// The endpoint id, as 64 lower-case hex characters.
    Id { endpoint_id: String },
    /// Text that is the whole output, such as a completion script. It is printed as it is with or
    /// without `--json`, because a script is not a JSON value.
    Text(String),
}

pub struct Output {
    format: Format,
    colour: Colour,
}

impl Output {
    pub fn new(format: Format, colour: Colour) -> Self {
        Self { format, colour }
    }

    pub fn format(&self) -> Format {
        self.format
    }

    /// Writes a result to stdout: in text, the line a person reads, and in JSON one object and a
    /// newline.
    pub fn print(&self, outcome: &Outcome) -> Result<(), Error> {
        let text = match (outcome, self.format) {
            (Outcome::Id { endpoint_id }, Format::Human) => format!("{endpoint_id}\n"),
            (Outcome::Id { endpoint_id }, Format::Json) => {
                format!("{}\n", json!({ "endpoint_id": endpoint_id }))
            }
            (Outcome::Text(text), _) => text.clone(),
        };
        let mut stdout = io::stdout().lock();
        // Not `println!`, which panics when it cannot write.
        match stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
        {
            Ok(()) => Ok(()),
            // `sluice id | head -c 0`: the reader has gone and wants nothing more, which is not a
            // failure of ours. Stop quietly. Any other write error is a failure of storage.
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(Error::Storage(format!(
                "cannot write to standard output: {e}"
            ))),
        }
    }

    /// Writes a failure to stderr and returns the exit code for it. In text it is
    /// `sluice: <message>`. In JSON it is one line, `{"error":{"kind":"policy","message":"..."}}`,
    /// and stdout stays empty because nothing was printed there.
    pub fn print_failure(&self, failure: &Failure) -> ExitCode {
        // A stderr that cannot be written to is nobody's to tell.
        let _ = io::stderr().write_all(failure_line(self.format, failure).as_bytes());
        ExitCode::from(exit_code(failure))
    }

    /// Prints what clap has to say, which is help, the version or a usage error, in the colour
    /// this process decided on. The exit code is clap's: 0 for help and version, 2 for a bad
    /// command line.
    pub fn print_clap(&self, error: &clap::Error) -> ExitCode {
        // clap styles the text with escape codes. `AutoStream` keeps them or strips them, and on
        // Windows it turns them on in a console that has them off.
        let text = error.render().ansi().to_string();
        let choice = |on| {
            if on {
                ColorChoice::Always
            } else {
                ColorChoice::Never
            }
        };
        let _ = if error.use_stderr() {
            AutoStream::new(io::stderr(), choice(self.colour.stderr)).write_all(text.as_bytes())
        } else {
            AutoStream::new(io::stdout(), choice(self.colour.stdout)).write_all(text.as_bytes())
        };
        ExitCode::from(u8::try_from(error.exit_code()).unwrap_or(2))
    }
}

/// The line for a failure, with its newline. `serde_json` writes the message, so a quote, a
/// backslash, a newline or a control character in it is escaped and the line stays valid JSON.
fn failure_line(format: Format, failure: &Failure) -> String {
    match format {
        Format::Human => format!("sluice: {}\n", text(failure)),
        Format::Json => {
            let object = json!({ "error": { "kind": kind(failure), "message": message(failure) } });
            format!("{object}\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use sluice_core::Error;

    use super::{Failure, Format, failure_line};

    #[test]
    fn a_json_failure_is_one_valid_line_whatever_the_message_holds() {
        let messages = [
            "plain",
            "a \"quoted\" word",
            "back\\slash and C:\\Users\\x",
            "two\nlines\r\nand a tab\t",
            "control \u{1} \u{7} \u{1f} and \u{7f}",
            "unicode \u{e9} \u{1f600} \u{2028}",
            "",
        ];
        for text in messages {
            let line = failure_line(Format::Json, &Failure::Error(Error::Policy(text.into())));
            assert!(line.ends_with('\n'), "{line:?}");
            assert_eq!(line.matches('\n').count(), 1, "{line:?}");
            let parsed: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(parsed["error"]["kind"], "policy");
            assert_eq!(parsed["error"]["message"], text);
            assert_eq!(parsed["error"].as_object().unwrap().len(), 2);
        }
    }

    #[test]
    fn a_text_failure_is_sluice_a_colon_and_the_message() {
        let line = failure_line(
            Format::Human,
            &Failure::Error(Error::Storage("disk full".into())),
        );
        assert_eq!(line, "sluice: storage error: disk full\n");
    }
}
