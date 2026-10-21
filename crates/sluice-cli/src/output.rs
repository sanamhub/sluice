//! Everything a command prints goes through here. A command returns its result as a value, and
//! this module decides how it looks: text or JSON, with colour or without. That keeps the rules
//! for scripts in one place, and `README.md` states them.

use std::io::{self, Write};
use std::process::ExitCode;

use anstream::{AutoStream, ColorChoice};
use serde_json::json;
use sluice_core::Error;

use crate::colour::Colour;
use crate::exit::{exit_code, kind, message};

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
        };
        let mut stdout = io::stdout().lock();
        // Not `println!`, which panics when it cannot write.
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|e| Error::Storage(format!("cannot write to standard output: {e}")))
    }

    /// Writes a failure to stderr and returns the exit code for it. In text it is
    /// `sluice: <message>`. In JSON it is one line, `{"error":{"kind":"policy","message":"..."}}`,
    /// and stdout stays empty because nothing was printed there.
    pub fn print_failure(&self, error: &Error) -> ExitCode {
        let line = match self.format {
            Format::Human => format!("sluice: {error}\n"),
            Format::Json => {
                let object = json!({ "error": { "kind": kind(error), "message": message(error) } });
                format!("{object}\n")
            }
        };
        // A stderr that cannot be written to is nobody's to tell.
        let _ = io::stderr().write_all(line.as_bytes());
        ExitCode::from(exit_code(error))
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
