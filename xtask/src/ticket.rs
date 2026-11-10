//! `cargo xtask ticket-parts`: the address a ticket names, as fields a script can read.
//!
//! The browser proof in `web/proof/` needs a ticket's sender, relay and root, and `sluice` has no
//! command that prints them: `inspect` describes what a ticket offers, not what it points at. This
//! is developer tooling and not a product command, so it lives here.

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

/// 64 lower-case hex characters, as a ticket and its parts are written.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use sluice_core::ticket::{Body, Format, Ticket};

    use super::run;

    /// The fields printed are the ones a ticket carries, and a live ticket says so.
    #[test]
    fn the_parts_are_the_ones_the_ticket_names() {
        let static_ticket = Ticket::new(
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
        .unwrap();
        assert!(run(&static_ticket.encode()).is_ok());

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
}
