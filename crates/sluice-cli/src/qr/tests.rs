//! The code is the ticket, so every test here reads the rendered block back with a decoder that
//! knows nothing about how it was drawn.

use std::net::{Ipv4Addr, SocketAddr};

use sluice_core::ticket::{Body, Format, Ticket};

use super::{LONGEST, TICKET_LIMIT, block, check_terminal, rows};

/// The text one rendered block carries. The block is read as the grid of modules it draws, two
/// module rows per text row, and handed to the decoder as greyscale: a dark module is black and a
/// light one is white, which is what a camera would see. Each module becomes a square of several
/// pixels, as a camera sees it: a decoder samples the code at picture resolution, and a code of the
/// largest size is 185 modules across the block.
fn read_back(rows: &[String]) -> String {
    const PIXELS: usize = 4;
    let grid: Vec<Vec<bool>> = rows
        .iter()
        .flat_map(|row| {
            let top: Vec<bool> = row.chars().map(|at| at == '█' || at == '▀').collect();
            let bottom: Vec<bool> = row.chars().map(|at| at == '█' || at == '▄').collect();
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
        if rows(&text).is_err() {
            break;
        }
        longest = Some(text);
    }
    longest.expect("a ticket of about two thousand characters fits")
}

/// The whole point: the block a person photographs carries the ticket, character for character.
#[test]
fn the_shortest_ticket_reads_back() {
    let ticket = shortest().encode();
    assert_eq!(read_back(&rows(&ticket).unwrap()), ticket);
}

#[test]
fn the_longest_ticket_a_code_holds_reads_back() {
    let ticket = longest();
    assert!(
        ticket.len() > LONGEST - 40,
        "the longest ticket found is {} characters",
        ticket.len()
    );
    assert_eq!(read_back(&rows(&ticket).unwrap()), ticket);
}

/// A ticket the standard allows and no code holds is refused by name, not truncated and not drawn
/// into a code that says something else.
#[test]
fn a_ticket_longer_than_a_code_holds_is_refused() {
    let ticket = shortest().encode();
    let padded = format!("{ticket}{}", "a".repeat(LONGEST));
    let error = block(&padded).unwrap_err().to_string();
    assert!(error.contains(&LONGEST.to_string()), "{error}");
    assert!(!error.contains(&ticket), "the refusal quotes the ticket");
    assert!(error.contains("--qr"), "{error}");
}

/// The capacity the refusal names is the real one: a code holds [`LONGEST`] characters of ticket
/// text and not one more, so the number in the message is not an estimate.
#[test]
fn a_code_holds_exactly_the_capacity_it_names() {
    assert!(rows(&"a".repeat(LONGEST)).is_ok());
    assert!(
        rows(&"a".repeat(LONGEST + 1)).is_err(),
        "a code holds more than {LONGEST} characters"
    );
}

/// Above the length a ticket can be at all, the refusal says so rather than naming the code's
/// capacity, which was never the limit.
#[test]
fn a_ticket_over_the_ticket_limit_says_so() {
    let error = block(&"a".repeat(TICKET_LIMIT + 1))
        .unwrap_err()
        .to_string();
    assert!(error.contains(&TICKET_LIMIT.to_string()), "{error}");
}

/// The block is framed in black on white so the dark modules are dark on either terminal theme.
#[test]
fn the_block_is_framed_in_black_on_white() {
    let block = block(&shortest().encode()).unwrap();
    assert!(block.starts_with("\u{1b}[30;107m"), "{block:?}");
    assert!(block.ends_with("\u{1b}[0m\n"), "{block:?}");
}

/// The quiet zone is the four modules the standard asks for, on every side.
#[test]
fn the_quiet_zone_is_four_modules_wide() {
    let rows = rows(&shortest().encode()).unwrap();
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
