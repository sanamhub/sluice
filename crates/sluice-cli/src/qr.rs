//! A ticket as a QR code, so a phone or a second screen can read it.
//!
//! The code goes to standard error and the ticket line stays on standard output, because the two
//! kinds of reader are different: a script reads the line, a person looks at the screen. A code is
//! drawn with half blocks, one character for two modules, inside the quiet zone the standard asks
//! for, and it carries the recovery level a screen needs.
//!
//! With colour on, the dark modules are written as dark characters on an explicit white field, so
//! the code reads the same on a light theme and on a dark one. With colour off there are no escapes
//! to set that field, so the drawing follows the terminal's background: on a light one the field is
//! the background and a dark module is ink, and on a dark one it is the other way round, so a dark
//! module is still a dark pixel.
//!
//! The background is read from `COLORFGBG` when the terminal sets it. Most terminals do not set it,
//! so a terminal that says nothing gets the drawing that reads whatever its background is: the
//! escaped one where escapes are allowed, which sets the field itself, and the light mapping plus one
//! line about `--qr=inverted` where they are not.

use qrcodegen::{QrCode, QrCodeEcc};
use sluice_core::Error;
use terminal_size::{Width, terminal_size_of};

use crate::output::Output;

/// The quiet zone around the code, in modules, which the standard requires and a reader needs to
/// find the code's edge.
const QUIET: i32 = 4;

/// The recovery level: a screen is not smudged, but a phone reads it at an angle and across glare,
/// and the medium level recovers about 15% of the code for that. The largest code at this level
/// holds [`LONGEST`] characters, which is many times a ticket.
const ECC: QrCodeEcc = QrCodeEcc::Medium;

/// The most characters one code holds at [`ECC`], in the byte mode a ticket's lower-case text is
/// written in: version 40 of the standard, 2331 bytes. A test checks the boundary.
pub const LONGEST: usize = 2331;

/// The most a ticket can be, from `SPEC.md` section 4, which is longer than one code holds.
const TICKET_LIMIT: usize = 4096;

/// How the code is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// Escape codes with a white field: a dark module is drawn in black ink.
    EscapedLight,
    /// Escape codes with a black field: a dark module is left as the field.
    EscapedDark,
    /// No escape codes on a light background: a dark module is the terminal's ink.
    OnLight,
    /// No escape codes on a dark background: a dark module is the terminal's background, so the
    /// field is ink and the code keeps dark modules dark.
    OnDark,
}

impl Ink {
    /// Whether the drawing sets its field with escape codes, which is what makes it readable on any
    /// theme.
    fn escapes(self) -> bool {
        matches!(self, Self::EscapedLight | Self::EscapedDark)
    }

    /// Whether the field is dark, which is what decides which of the two module drawings is used.
    fn dark_field(self) -> bool {
        matches!(self, Self::EscapedDark | Self::OnDark)
    }
}

/// Which drawing a run asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Ask {
    /// The one the terminal's background suggests, and the escaped one when it says nothing.
    #[default]
    Auto,
    /// The one for a dark background: light modules on a black field where escapes are allowed, and
    /// a drawn field where they are not.
    Inverted,
}

/// Refuses `--qr` on a stream that is not a terminal: a code written into a file or a pipe is
/// block characters in a log, and the ticket it carries is a secret.
pub fn check_terminal(terminal: bool) -> Result<(), Error> {
    if terminal {
        return Ok(());
    }
    Err(Error::Policy(
        "--qr draws a code for a person to look at, and standard error is not a terminal here; \
         run it in a terminal, or leave --qr out and pass the ticket on"
            .to_string(),
    ))
}

/// Writes the code for `ticket` to standard error, or refuses when the ticket is longer than one
/// code holds or the code is wider than the terminal. The message names limits and never the ticket:
/// a ticket is a secret.
pub fn write(output: &Output, ticket: &str, asked: Ask) -> Result<(), Error> {
    let colours = output.colours_stderr();
    let background = background();
    let ink = ink(asked, colours, background);
    let block = block(ticket, ink, columns())?;
    output.write_stderr_ink(&block, ink.escapes());
    if hint(asked, colours, background) {
        // The code was drawn for a light background because the terminal did not say which one it
        // has. A dark one shows the same code inverted, so a reader that needs the light field
        // cannot see it; the line says which flag draws the other way.
        output.write_stderr(
            "if the code does not scan, this terminal's background is dark: run the same command \
             with --qr=inverted\n",
        );
    }
    Ok(())
}

/// The ink this process draws with: the one asked for, or the one the terminal's background suggests
/// and escapes never need to be guessed.
fn ink(asked: Ask, colours: bool, background: Option<bool>) -> Ink {
    match asked {
        Ask::Inverted if colours => Ink::EscapedDark,
        Ask::Inverted => Ink::OnDark,
        Ask::Auto if colours => Ink::EscapedLight,
        Ask::Auto => match background {
            Some(true) => Ink::OnDark,
            Some(false) | None => Ink::OnLight,
        },
    }
}

/// Whether the code needs the line that names `--qr=inverted`: the light mapping was drawn only
/// because the terminal says nothing about its background, and the drawing could not say it either.
fn hint(asked: Ask, colours: bool, background: Option<bool>) -> bool {
    asked == Ask::Auto && !colours && background.is_none()
}

/// The code for `ticket` as text in `ink`, or the refusal when it does not fit. `columns` is the
/// terminal's width when it is known, and a code wider than it is refused rather than wrapped.
pub fn block(ticket: &str, ink: Ink, columns: Option<usize>) -> Result<String, Error> {
    let rows = rows(ticket, ink)?;
    let wide = rows.first().map_or(0, |row| row.chars().count());
    if let Some(columns) = columns
        && wide > columns
    {
        return Err(too_wide(wide, columns));
    }
    let field = if ink.escapes() {
        let pair = if ink.dark_field() {
            "\x1b[97;40m"
        } else {
            "\x1b[30;107m"
        };
        // The field is set and cleared on every row: a terminal, a pager or a line-wise reader that
        // clears the attributes at a line ending leaves a row that carries the code unreadable
        // otherwise.
        rows.iter()
            .map(|row| format!("{pair}{row}\x1b[0m"))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        rows.join("\n")
    };
    Ok(format!("{field}\n"))
}

/// One string per text row of the code, each holding two module rows and the quiet zone.
///
/// A ticket is lower-case base32, which no QR mode but the byte mode can carry, so the length that
/// fits is the byte capacity of the largest code at [`ECC`].
pub fn rows(ticket: &str, ink: Ink) -> Result<Vec<String>, Error> {
    let code = QrCode::encode_text(ticket, ECC).map_err(|_| too_long(ticket.len()))?;
    let size = code.size();
    let field_is_ink = ink.dark_field();
    let mut rows = Vec::new();
    let mut top = -QUIET;
    while top < size + QUIET {
        let mut row = String::new();
        for column in -QUIET..size + QUIET {
            let upper = dark(&code, column, top);
            let lower = dark(&code, column, top + 1);
            row.push(match (upper, lower, field_is_ink) {
                // On a dark field the light modules are drawn and a dark one is left as the field,
                // so the modules keep their luminance.
                (false, false, true) => '█',
                (false, true, true) => '▀',
                (true, false, true) => '▄',
                (true, true, true) => ' ',
                (true, true, false) => '█',
                (true, false, false) => '▀',
                (false, true, false) => '▄',
                (false, false, false) => ' ',
            });
        }
        rows.push(row);
        top += 2;
    }
    Ok(rows)
}

/// Whether the module at `column`, `row` of the code is dark. Every module of the quiet zone, and
/// the half row a code of an odd height leaves empty, is light.
fn dark(code: &QrCode, column: i32, row: i32) -> bool {
    (0..code.size()).contains(&column)
        && (0..code.size()).contains(&row)
        && code.get_module(column, row)
}

/// Whether the terminal says its background is dark, and `None` when it says nothing. `COLORFGBG` is
/// `foreground;background`, and the background is the last number, with 0 to 6 and 8 the dark
/// colours.
fn background() -> Option<bool> {
    let value = std::env::var_os("COLORFGBG")?;
    parse_background(&value)
}

/// [`background`] for one value of the variable, which is what a test has.
fn parse_background(value: &std::ffi::OsStr) -> Option<bool> {
    let text = value.to_string_lossy();
    let last = text.rsplit(';').next()?;
    Some(matches!(last.parse::<u8>(), Ok(0..=6 | 8)))
}

/// The terminal's width in columns: the width of the terminal standard error is attached to, which
/// is the one the code is drawn on, and `COLUMNS` only when there is no terminal to ask. A shell that
/// does not export `COLUMNS` leaves the width unknown where there is no terminal, and a code of an
/// unknown width is drawn rather than refused.
fn columns() -> Option<usize> {
    width(
        terminal_size_of(std::io::stderr()).map(|(Width(width), _)| usize::from(width)),
        std::env::var("COLUMNS")
            .ok()
            .and_then(|text| text.parse().ok()),
    )
}

/// The width a code may use: the terminal's own, and `COLUMNS` only when no terminal answered.
fn width(terminal: Option<usize>, columns: Option<usize>) -> Option<usize> {
    terminal.or(columns)
}

/// The refusal for a ticket that no code this size holds. It says how long the ticket is, which is
/// not its content, and what to do instead.
fn too_long(length: usize) -> Error {
    let hint = if length > TICKET_LIMIT {
        format!("a ticket is at most {TICKET_LIMIT} characters, so this is not one")
    } else {
        format!("a code at this recovery level holds {LONGEST}")
    };
    Error::Policy(format!(
        "this ticket is {length} characters, and {hint}; leave --qr out and pass the ticket on as \
         text"
    ))
}

/// The refusal for a code the terminal is too narrow to show whole. A wrapped code is not a code,
/// and a truncated one says something else.
fn too_wide(width: usize, columns: usize) -> Error {
    Error::Policy(format!(
        "this code is {width} columns wide and the terminal is {columns}; widen the terminal, or \
         leave --qr out and pass the ticket on as text"
    ))
}

#[cfg(test)]
mod tests;
