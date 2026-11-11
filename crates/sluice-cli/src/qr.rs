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
//! module is still a dark pixel. The background is read from `COLORFGBG` when the terminal sets it,
//! and a light one is assumed otherwise.

use qrcodegen::{QrCode, QrCodeEcc};
use sluice_core::Error;

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
    /// Escape codes: black modules on an explicit white field, the same on every theme.
    Escaped,
    /// No escape codes on a light background: a dark module is the terminal's ink.
    OnLight,
    /// No escape codes on a dark background: a dark module is the terminal's background, so the
    /// field is ink and the code keeps dark modules dark.
    OnDark,
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
pub fn write(output: &Output, ticket: &str) -> Result<(), Error> {
    let ink = ink(output);
    let block = block(ticket, ink, columns())?;
    output.write_stderr_ink(&block, ink == Ink::Escaped);
    Ok(())
}

/// The ink this process draws with: escapes when stderr has colour, and otherwise whichever mapping
/// keeps a dark module dark on the terminal's background.
fn ink(output: &Output) -> Ink {
    if output.colours_stderr() {
        Ink::Escaped
    } else if dark_background(std::env::var_os("COLORFGBG").as_deref()) {
        Ink::OnDark
    } else {
        Ink::OnLight
    }
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
    let field = rows.join("\n");
    Ok(if ink == Ink::Escaped {
        format!("\x1b[30;107m{field}\x1b[0m\n")
    } else {
        format!("{field}\n")
    })
}

/// One string per text row of the code, each holding two module rows and the quiet zone.
///
/// A ticket is lower-case base32, which no QR mode but the byte mode can carry, so the length that
/// fits is the byte capacity of the largest code at [`ECC`].
pub fn rows(ticket: &str, ink: Ink) -> Result<Vec<String>, Error> {
    let code = QrCode::encode_text(ticket, ECC).map_err(|_| too_long(ticket.len()))?;
    let size = code.size();
    let mut rows = Vec::new();
    let mut top = -QUIET;
    while top < size + QUIET {
        let mut row = String::new();
        for column in -QUIET..size + QUIET {
            let upper = dark(&code, column, top);
            let lower = dark(&code, column, top + 1);
            row.push(match (upper, lower, ink) {
                // On a dark background the field is ink: a light module is drawn and a dark one is
                // left as the background, so the modules keep their luminance.
                (false, false, Ink::OnDark) => '█',
                (false, true, Ink::OnDark) => '▀',
                (true, false, Ink::OnDark) => '▄',
                (true, true, Ink::OnDark) => ' ',
                (true, true, _) => '█',
                (true, false, _) => '▀',
                (false, true, _) => '▄',
                (false, false, _) => ' ',
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

/// Whether `COLORFGBG` says the terminal's background is dark. The variable is
/// `foreground;background` and the background is the last number, with 0 to 6 and 8 the dark
/// colours. A terminal that does not set it is taken as light, which is the case a reader that
/// cannot invert the picture needs.
fn dark_background(value: Option<&std::ffi::OsStr>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let text = value.to_string_lossy();
    let Some(background) = text.rsplit(';').next() else {
        return false;
    };
    matches!(background.parse::<u8>(), Ok(0..=6 | 8))
}

/// The terminal's width in columns, when the environment names it. `COLUMNS` is what a program
/// without a terminal query has, and a shell that does not export it leaves the width unknown.
fn columns() -> Option<usize> {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|text| text.parse().ok())
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
