//! Everything a command prints goes through here. A command returns its result as a value, and
//! this module decides how it looks: text or JSON, with colour or without. That keeps the rules
//! for scripts in one place, and `README.md` states them.

use std::io::{self, Write};
use std::process::ExitCode;

use anstream::{AutoStream, ColorChoice};
use serde_json::json;
use sluice_core::Error;
use sluice_core::engine::Offering;
use sluice_core::manifest::path::shown;

use crate::colour::Colour;
use crate::doctor::{self, Line};
use crate::exit::{Failure, exit_code, kind, message, text};

/// How results and failures are written. It follows `--json` and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Human,
    Json,
}

/// The bytes of a transfer in the units a person reads.
pub fn bytes_text(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} bytes")
    }
}

/// The line that says how much a transfer moves: `3 files, 2 folders, 1.2 GiB`. A transfer with no
/// folders or links says only the files and the size, and one file says `1 file`.
pub fn summary(files: u64, folders: u64, links: u64, bytes: u64) -> String {
    let mut line = format!("{files} {}", plural(files, "file", "files"));
    if folders > 0 {
        line.push_str(&format!(
            ", {folders} {}",
            plural(folders, "folder", "folders")
        ));
    }
    if links > 0 {
        line.push_str(&format!(", {links} {}", plural(links, "link", "links")));
    }
    line.push_str(&format!(", {}", bytes_text(bytes)));
    line
}

fn plural(count: u64, one: &'static str, many: &'static str) -> &'static str {
    if count == 1 { one } else { many }
}

/// What a command produced.
pub enum Outcome {
    /// The endpoint id, as 64 lower-case hex characters.
    Id { endpoint_id: String },
    /// Text that is the whole output, such as a completion script. It is printed as it is with or
    /// without `--json`, because a script is not a JSON value.
    Text(String),
    /// The report of `sluice doctor`, and the exit code it ends with.
    Doctor { lines: Vec<Line>, code: u8 },
    /// What `sluice inspect` read from a ticket, and whether every name is shown.
    Offering { offering: Offering, all: bool },
}

impl Outcome {
    /// The exit code once the outcome is printed: 0, except for a doctor report with a failed
    /// check.
    pub fn code(&self) -> u8 {
        match self {
            Outcome::Doctor { code, .. } => *code,
            _ => 0,
        }
    }
}

#[derive(Clone, Copy)]
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
        let (text, coloured) = match (outcome, self.format) {
            (Outcome::Id { endpoint_id }, Format::Human) => (format!("{endpoint_id}\n"), false),
            (Outcome::Id { endpoint_id }, Format::Json) => (
                format!("{}\n", json!({ "endpoint_id": endpoint_id })),
                false,
            ),
            (Outcome::Text(text), _) => (text.clone(), false),
            (Outcome::Doctor { lines, .. }, Format::Human) => {
                (doctor::human(lines, self.colour.stdout), self.colour.stdout)
            }
            (Outcome::Doctor { lines, .. }, Format::Json) => (doctor::json(lines), false),
            (Outcome::Offering { offering, all }, Format::Human) => {
                (crate::inspect::text(offering, *all), false)
            }
            (Outcome::Offering { offering, .. }, Format::Json) => {
                (crate::inspect::json(offering), false)
            }
        };
        // Through `AutoStream` only when there are escape codes, which it turns on in a Windows
        // console that has them off. Everything else goes out as it is.
        let written = if coloured {
            let mut stdout = AutoStream::new(io::stdout().lock(), ColorChoice::Always);
            stdout
                .write_all(text.as_bytes())
                .and_then(|()| stdout.flush())
        } else {
            let mut stdout = io::stdout().lock();
            // Not `println!`, which panics when it cannot write.
            stdout
                .write_all(text.as_bytes())
                .and_then(|()| stdout.flush())
        };
        match written {
            Ok(()) => Ok(()),
            // `sluice id | head -c 0`: the reader has gone and wants nothing more, which is not a
            // failure of ours. Stop quietly. Any other write error is a failure of storage.
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(Error::Storage(format!(
                "cannot write to standard output: {e}"
            ))),
        }
    }

    /// Writes text to stdout exactly as given. A broken pipe is not a failure.
    pub fn write_stdout(&self, text: &str) -> Result<(), Error> {
        let mut out = io::stdout().lock();
        match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
            Err(e) => Err(Error::Storage(format!(
                "cannot write to standard output: {e}"
            ))),
        }
    }

    /// Writes text to stderr. Nothing a person reads on stderr is worth failing a transfer for.
    pub fn write_stderr(&self, text: &str) {
        let _ = io::stderr().lock().write_all(text.as_bytes());
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

/// The line for a failure, with its newline. In text, every character the message did not have to
/// hold is written as its escape: a failure from the network carries the reason the peer closed the
/// connection with, and a terminal would obey an escape sequence or a bidirectional character in it.
/// The escape belongs here, at the one place a failure is written, so an error added later cannot
/// miss it. JSON is left alone: `serde_json` quotes and escapes the control characters that would
/// break the line, a bidirectional or line-separator character in it is valid JSON, and the line is
/// for a program rather than a terminal.
fn failure_line(format: Format, failure: &Failure) -> String {
    match format {
        Format::Human => format!("sluice: {}", line(&text(failure))),
        Format::Json => {
            let object = json!({ "error": { "kind": kind(failure), "message": message(failure) } });
            format!("{object}\n")
        }
    }
}

/// One line for a person, ending in its newline, with the text escaped by
/// [`sluice_core::manifest::path::shown`]. Every human line that carries something the CLI did not
/// write itself — an error's message, a warning, a name, a relay some peer named — is written
/// through here, so a terminal cannot be made to run an escape sequence or draw a line backwards.
pub fn line(text: &str) -> String {
    format!("{}\n", shown(text))
}

#[cfg(test)]
mod tests;
