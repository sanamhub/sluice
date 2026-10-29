//! Renders the transfer events as text on stderr, or as one JSON object per line on stdout.

use std::time::{Duration, Instant};

use sluice_core::{
    Error,
    engine::{Event, PathKind},
    ticket::fingerprint,
};
use tokio::{sync::mpsc::UnboundedReceiver, task::JoinHandle};

use crate::output::{Format, Output, bytes_text, line, summary};

/// How often a progress line is printed. Verification and path events are not throttled.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

/// Prints the events until the sender drops its end, then returns.
pub fn printer(
    output: Output,
    mut events: UnboundedReceiver<Event>,
    verb: &'static str,
) -> JoinHandle<Result<(), Error>> {
    tokio::spawn(async move {
        let mut last_progress: Option<Instant> = None;
        while let Some(event) = events.recv().await {
            if matches!(event, Event::Progress { .. })
                && last_progress.is_some_and(|last| last.elapsed() < PROGRESS_EVERY)
            {
                continue;
            }
            if matches!(event, Event::Progress { .. }) {
                last_progress = Some(Instant::now());
            }
            match output.format() {
                Format::Json => output.write_stdout(&format!("{}\n", event.json()))?,
                // An event can carry a name or a message a stranger chose, and it is written here
                // for people, so this is where it is escaped.
                Format::Human => output.write_stderr(&line(&human(&event, verb))),
            }
        }
        Ok(())
    })
}

/// Prints the ticket on stdout, alone in text mode and as one event line in JSON mode.
pub fn ticket(output: Output, ticket: &str) -> Result<(), Error> {
    match output.format() {
        Format::Human => output.write_stdout(&format!("{ticket}\n")),
        Format::Json => {
            let line = serde_json::json!({ "event": "ticket", "ticket": ticket });
            output.write_stdout(&format!("{line}\n"))
        }
    }
}

fn human(event: &Event, verb: &str) -> String {
    match event {
        Event::Planned {
            files,
            folders,
            links,
            bytes,
            of_files,
            of_bytes,
            fetched,
        } => planned(
            *files, *folders, *links, *bytes, *of_files, *of_bytes, *fetched,
        ),
        Event::Started {
            name,
            files,
            bytes,
            congestion,
        } => {
            let what = match name {
                Some(name) => format!("{verb} {name}"),
                None => format!("{verb} {files} files"),
            };
            format!("{what} ({}) with {congestion}", bytes_text(*bytes),)
        }
        Event::Connected { path, relay } => format!("connected {}", path_text(*path, relay)),
        Event::PathChanged { path, relay } => {
            format!("path changed: {}", path_text(*path, relay))
        }
        Event::Relayed { relay, seconds } => format!(
            "no direct path after {seconds} s; relayed through {relay}. A meeting point carries it \
             at 64 KiB/s; a full relay at its own limit."
        ),
        Event::Progress { bytes, total } => {
            let percent = bytes
                .saturating_mul(100)
                .checked_div(*total)
                .unwrap_or(0)
                .min(100);
            format!(
                "{percent}% {} of {}",
                bytes_text(*bytes),
                bytes_text(*total)
            )
        }
        Event::Retrying { attempt, seconds } => {
            format!("connection lost; retrying in {seconds} s (attempt {attempt})")
        }
        Event::Warning { message } => format!("warning: {message}"),
        Event::Verified { bytes } => format!("verified {}", bytes_text(*bytes)),
        Event::Done { bytes, seconds } => {
            format!("done, {} in {seconds:.1} s", bytes_text(*bytes))
        }
        Event::ReceiverStarted { endpoint, relayed } => format!(
            "receiver {} connected {}",
            fingerprint(endpoint),
            path_words(*relayed)
        ),
        Event::ReceiverFinished {
            endpoint,
            relayed,
            whole,
        } => {
            let what = if *whole {
                "finished, the whole transfer"
            } else {
                "dropped, part of the transfer"
            };
            format!(
                "receiver {} {what} ({})",
                fingerprint(endpoint),
                path_words(*relayed)
            )
        }
    }
}

/// How a receiver reached us, for a line that already says it is about a receiver.
fn path_words(relayed: bool) -> &'static str {
    if relayed {
        "through a relay"
    } else {
        "directly"
    }
}

/// The line for what a transfer will write: the summary, what part of the ticket a selection takes,
/// and the bytes the transfer moves when a pack makes those differ.
#[allow(clippy::too_many_arguments)]
fn planned(
    files: u64,
    folders: u64,
    links: u64,
    bytes: u64,
    of_files: Option<u64>,
    of_bytes: Option<u64>,
    fetched: Option<u64>,
) -> String {
    let mut line = summary(files, folders, links, bytes);
    // A selection says what part of the ticket it takes, so a user sees that the patterns did
    // something before the transfer starts.
    if let (Some(of_files), Some(of_bytes)) = (of_files, of_bytes) {
        line.push_str(&format!(" of {of_files} files, {}", bytes_text(of_bytes)));
    }
    // A packed file brings its whole pack, so the bytes moved can be more than the bytes written;
    // the line says how many, because that is what the transfer waits for.
    if let Some(fetched) = fetched {
        line.push_str(&format!(", {} to fetch", bytes_text(fetched)));
    }
    line
}

fn path_text(path: PathKind, relay: &Option<String>) -> String {
    match (path, relay) {
        (PathKind::Relayed, Some(relay)) => format!("relayed through {relay}"),
        (PathKind::Relayed, None) => "relayed".to_string(),
        (PathKind::Direct, _) => "direct".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use sluice_core::engine::{Congestion, Event, PathKind};

    use super::human;
    use crate::output::line;

    #[test]
    fn a_relay_is_named_and_a_direct_path_is_not() {
        assert_eq!(
            human(
                &Event::Connected {
                    path: PathKind::Relayed,
                    relay: Some("https://relay.example/".into())
                },
                "sending"
            ),
            "connected relayed through https://relay.example/"
        );
        assert_eq!(
            human(
                &Event::Connected {
                    path: PathKind::Direct,
                    relay: None
                },
                "receiving"
            ),
            "connected direct"
        );
    }

    /// What a transfer will write is one line, the same one `inspect` prints, and a name in it
    /// cannot change what the terminal shows.
    #[test]
    fn a_planned_transfer_is_the_line_inspect_prints() {
        let planned = Event::Planned {
            files: 3,
            folders: 2,
            links: 1,
            bytes: 1536,
            of_files: None,
            of_bytes: None,
            fetched: None,
        };
        assert_eq!(
            human(&planned, "receiving"),
            "3 files, 2 folders, 1 link, 1.5 KiB"
        );
        let started = Event::Started {
            name: Some("report\u{202e}fdp.exe".into()),
            files: 1,
            bytes: 1536,
            congestion: Congestion::Cubic,
        };
        assert_eq!(
            line(&human(&started, "receiving")),
            "receiving report\\u{202e}fdp.exe (1.5 KiB) with cubic\n"
        );
    }

    /// A selection that fetches a whole pack for a small file says what part of the ticket it takes
    /// and, after it, how many bytes the transfer moves: the two are different numbers.
    #[test]
    fn a_selection_that_fetches_more_than_it_writes_says_so() {
        let planned = Event::Planned {
            files: 1,
            folders: 1,
            links: 0,
            bytes: 5,
            of_files: Some(3),
            of_bytes: Some(40),
            fetched: Some(30),
        };
        assert_eq!(
            human(&planned, "receiving"),
            "1 file, 1 folder, 5 bytes of 3 files, 40 bytes, 30 bytes to fetch"
        );
        let same = Event::Planned {
            files: 1,
            folders: 1,
            links: 0,
            bytes: 5,
            of_files: Some(3),
            of_bytes: Some(40),
            fetched: None,
        };
        assert_eq!(
            human(&same, "receiving"),
            "1 file, 1 folder, 5 bytes of 3 files, 40 bytes"
        );
    }

    /// A relay is named in one line, and a warning is a line of its own: it too can hold a message
    /// a stranger wrote, so it is escaped at the same boundary as a name.
    #[test]
    fn a_warning_escapes_the_text_a_stranger_wrote() {
        let warning = Event::Warning {
            message: "the peer said \u{1b}[2J and \u{202e}, then\u{2028}a break".into(),
        };
        assert_eq!(
            line(&human(&warning, "receiving")),
            "warning: the peer said \\u{1b}[2J and \\u{202e}, then\\u{2028}a break\n"
        );
    }

    /// A transfer carried by a relay says so in one line, naming both kinds of relay: the client
    /// cannot know which one it reached, and a user deciding whether to wait needs both.
    #[test]
    fn a_relayed_transfer_names_the_relay_and_both_kinds_of_limit() {
        let text = human(
            &Event::Relayed {
                relay: "https://relay.example/".into(),
                seconds: 10,
            },
            "sending",
        );
        assert_eq!(
            text,
            "no direct path after 10 s; relayed through https://relay.example/. A meeting point \
             carries it at 64 KiB/s; a full relay at its own limit."
        );
    }
}
