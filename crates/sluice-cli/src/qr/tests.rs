//! The code is the ticket, so every test here reads the rendered block back with a decoder that
//! knows nothing about how it was drawn.

use std::{
    ffi::OsStr,
    net::{Ipv4Addr, SocketAddr},
};

use sluice_core::ticket::{Body, Format, Ticket};

use super::{
    Ask, Ink, LONGEST, TICKET_LIMIT, block, check_terminal, hint, parse_background, rows, width,
};

/// The text one rendered block carries. The block is read as the grid of modules it draws, two
/// module rows per text row, and handed to the decoder as greyscale: a dark module is black and a
/// light one is white, which is what a camera would see. Each module becomes a square of several
/// pixels, as a camera sees it: a decoder samples the code at picture resolution, and a code of the
/// largest size is 185 modules across the block.
fn read_back(rows: &[String], ink: Ink) -> String {
    const PIXELS: usize = 4;
    // A dark field draws the light modules, so a drawn block is a light one there.
    let field_is_ink = ink.dark_field();
    let grid: Vec<Vec<bool>> = rows
        .iter()
        .flat_map(|row| {
            let top: Vec<bool> = row
                .chars()
                .map(|at| {
                    if field_is_ink {
                        at == ' ' || at == '▄'
                    } else {
                        at == '█' || at == '▀'
                    }
                })
                .collect();
            let bottom: Vec<bool> = row
                .chars()
                .map(|at| {
                    if field_is_ink {
                        at == ' ' || at == '▀'
                    } else {
                        at == '█' || at == '▄'
                    }
                })
                .collect();
            [top, bottom]
        })
        .collect();
    let width = grid[0].len() * PIXELS;
    let height = grid.len() * PIXELS;
    let mut image = rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        if grid[y / PIXELS][x / PIXELS] { 0 } else { 255 }
    });
    let grids = image.detect_grids();
    assert_eq!(grids.len(), 1, "one code in the block");
    let (_meta, content) = grids[0].decode().expect("the code decodes");
    content
}

/// The shortest ticket this program writes: a raw root of no bytes, no address, no relay, no
/// expiry.
fn shortest() -> Ticket {
    Ticket::new(
        [1; 32],
        vec![],
        None,
        None,
        Body::Static {
            root: [2; 32],
            size: 0,
            format: Format::Raw,
        },
    )
    .expect("a static ticket with no address")
}

/// The longest ticket one code holds at the recovery level used, found by adding addresses until
/// the next one would not fit.
fn longest() -> String {
    let mut addrs: Vec<SocketAddr> = Vec::new();
    let mut longest = None;
    for index in 1..=TICKET_LIMIT as u32 {
        addrs.push(SocketAddr::from((
            Ipv4Addr::from(index),
            1024 + (index % 60000) as u16,
        )));
        let Ok(ticket) = Ticket::new(
            [1; 32],
            addrs.clone(),
            None,
            None,
            Body::Static {
                root: [2; 32],
                size: 1 << 20,
                format: Format::HashSequence,
            },
        ) else {
            break;
        };
        let text = ticket.encode();
        if rows(&text, Ink::OnLight).is_err() {
            break;
        }
        longest = Some(text);
    }
    longest.expect("a ticket of about two thousand characters fits")
}

/// The whole point: the block a person photographs carries the ticket, character for character,
/// whichever ink it was drawn in.
#[test]
fn the_shortest_ticket_reads_back() {
    let ticket = shortest().encode();
    for ink in [Ink::OnLight, Ink::OnDark] {
        assert_eq!(
            read_back(&rows(&ticket, ink).unwrap(), ink),
            ticket,
            "{ink:?}"
        );
    }
}

#[test]
fn the_longest_ticket_a_code_holds_reads_back() {
    let ticket = longest();
    assert!(
        ticket.len() > LONGEST - 40,
        "the longest ticket found is {} characters",
        ticket.len()
    );
    assert_eq!(
        read_back(&rows(&ticket, Ink::OnLight).unwrap(), Ink::OnLight),
        ticket
    );
}

/// A ticket the standard allows and no code holds is refused by name, not truncated and not drawn
/// into a code that says something else.
#[test]
fn a_ticket_longer_than_a_code_holds_is_refused() {
    let ticket = shortest().encode();
    let padded = format!("{ticket}{}", "a".repeat(LONGEST));
    let error = block(&padded, Ink::OnLight, None).unwrap_err().to_string();
    assert!(error.contains(&LONGEST.to_string()), "{error}");
    assert!(!error.contains(&ticket), "the refusal quotes the ticket");
    assert!(error.contains("--qr"), "{error}");
}

/// The capacity the refusal names is the real one: a code holds [`LONGEST`] characters of ticket
/// text and not one more, so the number in the message is not an estimate.
#[test]
fn a_code_holds_exactly_the_capacity_it_names() {
    assert!(rows(&"a".repeat(LONGEST), Ink::OnLight).is_ok());
    assert!(
        rows(&"a".repeat(LONGEST + 1), Ink::OnLight).is_err(),
        "a code holds more than {LONGEST} characters"
    );
}

/// Above the length a ticket can be at all, the refusal says so rather than naming the code's
/// capacity, which was never the limit.
#[test]
fn a_ticket_over_the_ticket_limit_says_so() {
    let error = block(&"a".repeat(TICKET_LIMIT + 1), Ink::OnLight, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains(&TICKET_LIMIT.to_string()), "{error}");
}

/// With colour, the block is framed in black on white so the dark modules are dark on either
/// terminal theme, and the frame is set again on every row: a terminal or a reader that clears the
/// attributes at a line ending leaves a row readable only if it carries its own.
#[test]
fn every_row_of_an_escaped_block_sets_and_clears_the_field() {
    let ticket = shortest().encode();
    let framed = block(&ticket, Ink::EscapedLight, None).unwrap();
    let rows: Vec<&str> = framed.lines().collect();
    assert!(rows.len() > 1, "{framed:?}");
    for row in &rows {
        assert!(row.starts_with("\u{1b}[30;107m"), "{row:?}");
        assert!(row.ends_with("\u{1b}[0m"), "{row:?}");
    }
    assert!(framed.ends_with('\n'), "{framed:?}");

    // The inverted drawing sets a black field and draws the light modules in it.
    let inverted = block(&ticket, Ink::EscapedDark, None).unwrap();
    for row in inverted.lines() {
        assert!(row.starts_with("\u{1b}[97;40m"), "{row:?}");
        assert!(row.ends_with("\u{1b}[0m"), "{row:?}");
    }
}

/// Both mappings read back as the ticket whichever field they are drawn for, and so do the two
/// escaped drawings, whose modules are the same characters as the plain ones.
#[test]
fn both_mappings_read_back() {
    let ticket = shortest().encode();
    for ink in [
        Ink::OnLight,
        Ink::OnDark,
        Ink::EscapedLight,
        Ink::EscapedDark,
    ] {
        assert_eq!(
            read_back(&rows(&ticket, ink).unwrap(), ink),
            ticket,
            "{ink:?}"
        );
    }
}

/// With colour off, no escape reaches the terminal, and each mapping still gives a code the
/// decoder reads: on a light background the field is the background, on a dark one it is ink.
#[test]
fn a_code_without_colour_has_no_escapes_and_still_reads() {
    let ticket = shortest().encode();
    for ink in [Ink::OnLight, Ink::OnDark] {
        let text = block(&ticket, ink, None).unwrap();
        assert!(!text.contains('\u{1b}'), "{ink:?}: {text:?}");
        let rows = rows(&ticket, ink).unwrap();
        assert_eq!(read_back(&rows, ink), ticket, "{ink:?}");
    }
}

/// On a dark background the field is drawn and a dark module is not, so the first row is all ink.
#[test]
fn a_dark_background_draws_the_field_in_ink() {
    let rows = rows(&shortest().encode(), Ink::OnDark).unwrap();
    let first = rows.first().unwrap();
    assert!(first.chars().all(|at| at == '█'), "{first:?}");
}

/// `COLORFGBG` is `foreground;background`, and the background decides. A terminal that does not set
/// it says nothing, which is different from saying it is light.
#[test]
fn a_dark_background_is_read_from_the_last_number() {
    assert_eq!(parse_background(OsStr::new("15;0")), Some(true));
    assert_eq!(parse_background(OsStr::new("7;8")), Some(true));
    assert_eq!(parse_background(OsStr::new("0;15")), Some(false));
    assert_eq!(parse_background(OsStr::new("0;7")), Some(false));
    assert_eq!(parse_background(OsStr::new("15")), Some(false));
}

/// With escapes, the field is set and the terminal's background does not matter; without them, the
/// mapping is the background's, and an unknown one gets the light mapping and the line about
/// `--qr=inverted`.
#[test]
fn the_mapping_follows_the_background_and_what_was_asked_for() {
    use super::ink;

    assert_eq!(ink(Ask::Auto, true, None), Ink::EscapedLight);
    assert_eq!(ink(Ask::Auto, true, Some(true)), Ink::EscapedLight);
    assert_eq!(ink(Ask::Inverted, true, None), Ink::EscapedDark);
    assert_eq!(ink(Ask::Auto, false, Some(true)), Ink::OnDark);
    assert_eq!(ink(Ask::Auto, false, Some(false)), Ink::OnLight);
    assert_eq!(ink(Ask::Auto, false, None), Ink::OnLight);
    assert_eq!(ink(Ask::Inverted, false, Some(false)), Ink::OnDark);

    assert!(
        hint(Ask::Auto, false, None),
        "an unknown background needs the line"
    );
    assert!(!hint(Ask::Auto, true, None), "escapes set the field");
    assert!(
        !hint(Ask::Auto, false, Some(false)),
        "the background is known"
    );
    assert!(!hint(Ask::Inverted, false, None), "the flag was given");
}

/// The width is the terminal's own when it has one, and `COLUMNS` is the fallback.
#[test]
fn the_width_is_the_terminals_and_then_the_variable() {
    assert_eq!(width(Some(120), Some(80)), Some(120));
    assert_eq!(width(None, Some(80)), Some(80));
    assert_eq!(width(None, None), None);
}

/// A code wider than the terminal is refused rather than wrapped or cut, and the refusal names the
/// two widths and never the ticket.
#[test]
fn a_code_wider_than_the_terminal_is_refused() {
    let ticket = shortest().encode();
    let error = block(&ticket, Ink::OnLight, Some(10))
        .unwrap_err()
        .to_string();
    assert!(error.contains("columns"), "{error}");
    assert!(error.contains("10"), "{error}");
    assert!(!error.contains(&ticket), "the refusal quotes the ticket");
}

/// The quiet zone is the four modules the standard asks for, on every side. It is the background on
/// a light one and the ink on a dark one, which is light either way.
#[test]
fn the_quiet_zone_is_four_modules_wide() {
    let rows = rows(&shortest().encode(), Ink::OnLight).unwrap();
    for row in &rows[..2] {
        assert_eq!(row.chars().count(), rows[0].chars().count());
        assert!(row.chars().all(|at| at == ' '), "{row:?}");
    }
    for row in &rows[2..] {
        assert!(
            row.chars().take(4).all(|at| at == ' '),
            "the left quiet zone is short: {row:?}"
        );
    }
    for row in &rows {
        let right: Vec<char> = row.chars().rev().take(4).collect();
        assert!(right.iter().all(|at| *at == ' '), "{row:?}");
    }
}

/// A code written where a person is not looking is refused, with the reason and the way out.
#[test]
fn a_stream_that_is_not_a_terminal_is_refused() {
    assert!(check_terminal(true).is_ok());
    let error = check_terminal(false).unwrap_err().to_string();
    assert!(error.contains("not a terminal"), "{error}");
    assert!(error.contains("--qr"), "{error}");
}
