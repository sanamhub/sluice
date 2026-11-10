//! A ticket as a QR code, so a phone or a second screen can read it.
//!
//! The code goes to standard error and the ticket line stays on standard output, because the two
//! kinds of reader are different: a script reads the line, a person looks at the screen. A code is
//! drawn with half blocks, one character for two modules, inside the quiet zone the standard asks
//! for, and it carries the recovery level a screen needs. The dark modules are written as dark
//! characters on an explicit white background rather than as whatever the terminal's foreground is,
//! so the code reads the same on a light theme and on a dark one.

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
/// code holds. The message names the limit and never the ticket: a ticket is a secret.
pub fn write(output: &Output, ticket: &str) -> Result<(), Error> {
    let block = block(ticket)?;
    output.write_stderr(&block);
    Ok(())
}

/// The code for `ticket` as text, framed in black on white, or the refusal when it does not fit.
pub fn block(ticket: &str) -> Result<String, Error> {
    let rows = rows(ticket)?;
    Ok(format!("\x1b[30;107m{}\x1b[0m\n", rows.join("\n")))
}

/// One string per text row of the code, each holding two module rows and the quiet zone.
///
/// A ticket is lower-case base32, which no QR mode but the byte mode can carry, so the length that
/// fits is the byte capacity of the largest code at [`ECC`].
pub fn rows(ticket: &str) -> Result<Vec<String>, Error> {
    let code = QrCode::encode_text(ticket, ECC).map_err(|_| too_long(ticket.len()))?;
    let size = code.size();
    let mut rows = Vec::new();
    let mut top = -QUIET;
    while top < size + QUIET {
        let mut row = String::new();
        for column in -QUIET..size + QUIET {
            let upper = dark(&code, column, top);
            let lower = dark(&code, column, top + 1);
            row.push(match (upper, lower) {
                (true, true) => '█',
                (true, false) => '▀',
                (false, true) => '▄',
                (false, false) => ' ',
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

#[cfg(test)]
mod tests;
