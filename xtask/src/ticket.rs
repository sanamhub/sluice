//! `cargo xtask ticket-parts`: the address a ticket names, as fields a script can read.
//!
//! The browser proof in `web/proof/` needs a ticket's sender, relay and root, and `sluice` has no
//! command that prints them: `inspect` describes what a ticket offers, not what it points at. This
//! is developer tooling and not a product command, so it lives here.
//!
//! The ticket is read from standard input rather than taken as an argument, so it is never a command
//! line that a process list or a shell history keeps. Its parts are the ticket by another spelling,
//! so the output is as secret as the ticket itself.

use std::io::Read;

use sluice_core::ticket::{Body, Ticket};

/// Prints one `field value` line per field of `text`, for the scripts that read a ticket's parts.
pub fn run(text: &str) -> anyhow::Result<()> {
    let ticket = Ticket::decode(text)?;
    println!("sender {}", hex(ticket.endpoint_id()));
    println!("relay {}", ticket.relay_url().unwrap_or_default());
    match ticket.body() {
        Body::Static { root, size, .. } => {
            println!("root {}", hex(root));
            println!("size {size}");
        }
        Body::Live { stream_id, .. } => {
            println!("kind live");
            println!("stream {}", hex(stream_id));
        }
    }
    Ok(())
}

/// Reads one ticket from standard input and prints its parts.
pub fn run_stdin() -> anyhow::Result<()> {
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    run(from_input(&text))
}

/// The text of one ticket read from a stream. A shell's line ending is not part of the ticket, and
/// the decoder refuses text with a byte after it, so it is trimmed before the decode.
fn from_input(text: &str) -> &str {
    text.trim_end()
}

/// The bytes as lower-case hex, two characters each: 64 for a root, a sender or a key, and 32 for a
/// stream id.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use sluice_core::ticket::{Body, Format, Ticket};

    use super::{from_input, run};

    /// A static ticket with one address and a relay, for the tests below.
    fn static_ticket() -> Ticket {
        Ticket::new(
            [1; 32],
            vec![SocketAddr::from(([127, 0, 0, 1], 41641))],
            Some("http://127.0.0.1:3340/".to_string()),
            None,
            Body::Static {
                root: [9; 32],
                size: 2 << 20,
                format: Format::HashSequence,
            },
        )
        .unwrap()
    }

    /// The fields printed are the ones a ticket carries, and a live ticket says so.
    #[test]
    fn the_parts_are_the_ones_the_ticket_names() {
        assert!(run(&static_ticket().encode()).is_ok());

        let live = Ticket::new(
            [1; 32],
            vec![],
            None,
            None,
            Body::Live {
                stream_id: [3; 16],
                sender_key: [3; 32],
            },
        )
        .unwrap();
        assert!(run(&live.encode()).is_ok());
        assert!(run("sluice1not-a-ticket").is_err());
    }

    /// A ticket piped in ends with a line ending that is not part of it, and the decoder refuses a
    /// byte after the ticket, so the input is trimmed first.
    #[test]
    fn a_line_ending_from_a_pipe_is_not_part_of_the_ticket() {
        let text = static_ticket().encode();
        assert_eq!(from_input(&format!("{text}\r\n")), text);
        assert_eq!(from_input(&format!("{text}\n")), text);
        assert_eq!(from_input(&text), text);
    }
}
