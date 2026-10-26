//! Renders the transfer events as text on stderr, or as one JSON object per line on stdout.

use std::time::{Duration, Instant};

use sluice_core::{
    Error,
    engine::{Event, PathKind},
};
use tokio::{sync::mpsc::UnboundedReceiver, task::JoinHandle};

use crate::output::{Format, Output};

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
                Format::Human => output.write_stderr(&format!("{}\n", human(&event, verb))),
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

fn bytes_text(bytes: u64) -> String {
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

#[cfg(test)]
mod tests {
    use sluice_core::engine::{Event, PathKind};

    use super::{bytes_text, human};

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

    #[test]
    fn sizes_use_binary_units_and_do_not_divide_by_zero() {
        assert_eq!(bytes_text(0), "0 bytes");
        assert_eq!(bytes_text(1536), "1.5 KiB");
        assert_eq!(bytes_text(100 << 20), "100.0 MiB");
    }
}
