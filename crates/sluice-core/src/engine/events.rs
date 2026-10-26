//! What a transfer reports while it runs. One type, so the text output, the JSON lines and the
//! control API added later cannot drift apart.

use std::fmt;

use iroh::endpoint::{Connection, PathList};
use n0_future::StreamExt;
use serde::Serialize;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

/// How a connection reaches the peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PathKind {
    /// A direct UDP path.
    Direct,
    /// The transfer goes through a relay.
    Relayed,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => f.write_str("direct"),
            Self::Relayed => f.write_str("relayed"),
        }
    }
}

/// One thing that happened during a transfer.
///
/// The variant name is the `event` field in JSON, so a consumer reads `started`, `connected`,
/// `path_changed`, `progress`, `verified` or `done`. No event carries a ticket.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The transfer knows what it is about to move. `name` is set for one file; a folder has
    /// `files` entries and no single name.
    Started {
        /// The file name, when the transfer is one file.
        name: Option<String>,
        /// How many files the transfer will write.
        files: u64,
        /// The sum of their sizes in bytes.
        bytes: u64,
    },
    /// The connection is up, and this is the path it uses.
    Connected {
        /// Direct or relayed.
        path: PathKind,
        /// The relay URL when the path is relayed.
        relay: Option<String>,
    },
    /// The selected path changed during the transfer.
    PathChanged {
        /// Direct or relayed.
        path: PathKind,
        /// The relay URL when the path is relayed.
        relay: Option<String>,
    },
    /// Bytes verified and written so far.
    Progress {
        /// Bytes done.
        bytes: u64,
        /// Bytes the transfer will move, as the ticket or manifest said.
        total: u64,
    },
    /// Every byte arrived and matched its root.
    Verified {
        /// Bytes verified.
        bytes: u64,
    },
    /// The transfer finished.
    Done {
        /// Bytes written.
        bytes: u64,
        /// Seconds from the first request to the last verified byte.
        seconds: f64,
    },
}

impl Event {
    /// Prints this event as one JSON object on its own line, for `--json`.
    pub fn json(&self) -> String {
        // The type is ours and holds no map or non-string key, so this cannot fail.
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// Sends an event if anyone is still listening. A closed receiver is not an error: the transfer
/// finishes even when its reporter has gone.
pub fn emit(tx: &UnboundedSender<Event>, event: Event) {
    let _ = tx.send(event);
}

/// The selected path of a connection, or the only path it has. iroh marks a path selected a little
/// after data first flows, so a short transfer can hold several paths and none selected.
fn current_path(conn: &Connection) -> Option<(PathKind, Option<String>)> {
    let list = conn.paths();
    let chosen = list
        .iter()
        .find(|path| path.is_selected())
        .or_else(|| list.iter().next())?;
    Some(path_of(&chosen))
}

fn path_of(path: &iroh::endpoint::Path<'_>) -> (PathKind, Option<String>) {
    if path.is_relay() {
        (
            PathKind::Relayed,
            Some(
                path.remote_addr()
                    .to_string()
                    .strip_prefix("relay:")
                    .unwrap_or_default()
                    .to_string(),
            ),
        )
    } else {
        (PathKind::Direct, None)
    }
}

/// Follows the connection's selected path until it closes, reporting the first one immediately and
/// every change after that. The returned task is detached when the receiver goes away.
pub fn watch_path(conn: Connection, tx: UnboundedSender<Event>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut last = None;
        if let Some((path, relay)) = current_path(&conn) {
            emit(
                &tx,
                Event::Connected {
                    path,
                    relay: relay.clone(),
                },
            );
            last = Some((path, relay));
        }
        let mut changes = conn.paths_stream();
        while let Some(list) = changes.next().await {
            let Some(path) = selected(&list) else {
                continue;
            };
            if last.as_ref() == Some(&path) {
                continue;
            }
            emit(
                &tx,
                Event::PathChanged {
                    path: path.0,
                    relay: path.1.clone(),
                },
            );
            last = Some(path);
        }
    })
}

fn selected(list: &PathList<'_>) -> Option<(PathKind, Option<String>)> {
    let path = list
        .iter()
        .find(|path| path.is_selected())
        .or_else(|| list.iter().next())?;
    Some(path_of(&path))
}

#[cfg(test)]
mod tests {
    use super::{Event, PathKind};

    #[test]
    fn json_names_the_variant_and_its_fields() {
        let event = Event::Connected {
            path: PathKind::Relayed,
            relay: Some("https://relay.example/".into()),
        };
        let json = event.json();
        assert!(json.contains(r#""event":"connected""#), "{json}");
        assert!(json.contains(r#""path":"relayed""#), "{json}");
        assert!(
            json.contains(r#""relay":"https://relay.example/""#),
            "{json}"
        );
    }

    #[test]
    fn a_started_event_for_one_file_keeps_its_name_out_of_the_type() {
        let event = Event::Started {
            name: Some("a.bin".into()),
            files: 1,
            bytes: 7,
        };
        assert_eq!(
            event.json(),
            r#"{"event":"started","name":"a.bin","files":1,"bytes":7}"#
        );
    }
}
