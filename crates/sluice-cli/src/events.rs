//! Renders the transfer events as text on stderr, or as one JSON object per line on stdout.

use std::time::{Duration, Instant};

use sluice_core::{
    Error,
    engine::{Event, PathKind},
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
        } => summary(*files, *folders, *links, *bytes),
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
    }
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
