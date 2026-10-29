//! What a transfer reports while it runs. One type, so the text output, the JSON lines and the
//! control API added later cannot drift apart.

use std::{fmt, time::Duration};

use iroh::endpoint::{Connection, PathList};
use n0_future::StreamExt;
use serde::Serialize;
use tokio::{sync::mpsc::UnboundedSender, task::JoinHandle};

use super::endpoint::Congestion;

/// How long a connection may go on without a direct path before the transfer says so. Ten seconds
/// is long enough for a hole punch that is going to work, and short enough that a stream carried by
/// a relay is explained before a user gives up on it.
pub const RELAY_NOTICE: Duration = Duration::from_secs(10);

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
/// The variant name is the `event` field in JSON, so a consumer reads `planned`, `started`,
/// `connected`, `path_changed`, `progress`, `verified` or `done`. No event carries a ticket.
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
        /// The congestion controller the transfer runs with.
        congestion: Congestion,
    },
    /// What the transfer is about to write, counted from the manifest before the first byte: the
    /// same line `inspect` prints, so a user sees the size before anything is written. A receive
    /// that takes only some of a folder's entries says here what part it takes: `of_files` and
    /// `of_bytes` are the whole manifest's, and both are set or neither is.
    Planned {
        /// How many files it will write.
        files: u64,
        /// How many folders it will make.
        folders: u64,
        /// How many symlinks it will make.
        links: u64,
        /// The sum of the files' sizes in bytes.
        bytes: u64,
        /// How many files the whole manifest holds, when only some are taken.
        #[serde(skip_serializing_if = "Option::is_none")]
        of_files: Option<u64>,
        /// The sum of every file's size in the manifest, when only some are taken.
        #[serde(skip_serializing_if = "Option::is_none")]
        of_bytes: Option<u64>,
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
    /// The connection has been up for a while and still has no direct path, so the transfer is
    /// carried by the relay named here. Nothing failed; a meeting point is fast enough to
    /// coordinate and slow enough that a file takes a while, and a full relay has its own limit.
    Relayed {
        /// The relay the transfer goes through.
        relay: String,
        /// How many seconds the connection waited for a direct path.
        seconds: u64,
    },
    /// Bytes verified and written so far.
    Progress {
        /// Bytes done.
        bytes: u64,
        /// Bytes the transfer will move, as the ticket or manifest said.
        total: u64,
    },
    /// The connection was lost and the transfer is about to try again.
    Retrying {
        /// Which retry this is, from 1.
        attempt: u32,
        /// How long it will wait first.
        seconds: u64,
    },
    /// Something the transfer could not do, which does not stop it.
    Warning {
        /// What was skipped, with no path from a manifest.
        message: String,
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
/// every change after that, and saying once when the connection has gone [`RELAY_NOTICE`] without a
/// direct path. The returned task is detached when the receiver goes away.
pub fn watch_path(conn: Connection, tx: UnboundedSender<Event>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut notice = RelayNotice::new();
        if let Some((path, relay)) = current_path(&conn) {
            emit(
                &tx,
                Event::Connected {
                    path,
                    relay: relay.clone(),
                },
            );
            notice.start((path, relay));
        }
        let mut changes = conn.paths_stream();
        loop {
            tokio::select! {
                event = notice.expired(), if notice.owed() => {
                    if let Some(event) = event {
                        emit(&tx, event);
                    }
                }
                list = changes.next() => {
                    let Some(list) = list else {
                        break;
                    };
                    let Some(path) = selected(&list) else {
                        continue;
                    };
                    if let Some(event) = notice.saw(path) {
                        emit(&tx, event);
                    }
                }
            }
        }
    })
}

/// The one relay line a connection may owe: a deadline of [`RELAY_NOTICE`] from the connection
/// coming up, the path last seen, and whether the line was already said. It is a value of its own so
/// the decision can be driven by a paused clock in a test.
struct RelayNotice {
    last: Option<(PathKind, Option<String>)>,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
    said: bool,
}

impl RelayNotice {
    fn new() -> Self {
        Self {
            last: None,
            deadline: Box::pin(tokio::time::sleep(RELAY_NOTICE)),
            said: false,
        }
    }

    /// The path the connection started on. It was already reported as `connected`.
    fn start(&mut self, path: (PathKind, Option<String>)) {
        self.last = Some(path);
    }

    /// The change event for a path the connection moved to, and none when it did not move.
    fn saw(&mut self, path: (PathKind, Option<String>)) -> Option<Event> {
        if self.last.as_ref() == Some(&path) {
            return None;
        }
        self.last = Some(path.clone());
        Some(Event::PathChanged {
            path: path.0,
            relay: path.1,
        })
    }

    /// Whether the deadline has something left to say. The watcher stops polling once it has.
    fn owed(&self) -> bool {
        !self.said
    }

    /// Waits for the deadline and returns the line it owes, if the path is still relayed then. A
    /// path that went direct before the deadline owes nothing.
    async fn expired(&mut self) -> Option<Event> {
        if self.said {
            return None;
        }
        (&mut self.deadline).await;
        self.said = true;
        match self.last.clone() {
            Some((PathKind::Relayed, Some(relay))) => Some(Event::Relayed {
                relay,
                seconds: RELAY_NOTICE.as_secs(),
            }),
            _ => None,
        }
    }
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
    use super::{Congestion, Event, PathKind, RELAY_NOTICE, RelayNotice};
    use std::time::Duration;

    const RELAY: &str = "https://relay.example/";

    fn relayed() -> (PathKind, Option<String>) {
        (PathKind::Relayed, Some(RELAY.to_string()))
    }

    fn direct() -> (PathKind, Option<String>) {
        (PathKind::Direct, None)
    }

    /// A connection that is still relayed at the mark says so once, with the relay and the wait, and
    /// never again: the line is about the path it could not find, and a second one says nothing new.
    #[tokio::test(start_paused = true)]
    async fn a_relayed_connection_is_named_once_at_the_mark() {
        let mut notice = RelayNotice::new();
        notice.start(relayed());
        tokio::time::advance(RELAY_NOTICE).await;
        assert_eq!(
            notice.expired().await,
            Some(Event::Relayed {
                relay: RELAY.to_string(),
                seconds: 10,
            })
        );
        assert_eq!(notice.expired().await, None, "the line was said twice");
    }

    /// A path that goes direct before the mark owes no line at all: it is the ordinary change, and
    /// the wait it reports is not one a user has to read about.
    #[tokio::test(start_paused = true)]
    async fn a_path_that_goes_direct_before_the_mark_says_nothing() {
        let mut notice = RelayNotice::new();
        notice.start(relayed());
        tokio::time::advance(Duration::from_secs(9)).await;
        assert_eq!(
            notice.saw(direct()),
            Some(Event::PathChanged {
                path: PathKind::Direct,
                relay: None,
            })
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(
            notice.expired().await,
            None,
            "a direct path was still called relayed"
        );
    }

    /// A path that goes direct after the mark gets both: the relay line it owed, and then the
    /// change to the path that arrived.
    #[tokio::test(start_paused = true)]
    async fn a_path_that_goes_direct_after_the_mark_gets_the_line_and_the_change() {
        let mut notice = RelayNotice::new();
        notice.start(relayed());
        tokio::time::advance(Duration::from_secs(12)).await;
        assert!(matches!(
            notice.expired().await,
            Some(Event::Relayed { .. })
        ));
        assert_eq!(
            notice.saw(direct()),
            Some(Event::PathChanged {
                path: PathKind::Direct,
                relay: None
            })
        );
        assert_eq!(notice.saw(direct()), None, "a path that did not move");
    }

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
            congestion: Congestion::Cubic,
        };
        assert_eq!(
            event.json(),
            r#"{"event":"started","name":"a.bin","files":1,"bytes":7,"congestion":"cubic"}"#
        );
    }

    #[test]
    fn a_started_event_names_the_controller_that_runs() {
        let event = Event::Started {
            name: Some("a.bin".into()),
            files: 1,
            bytes: 7,
            congestion: Congestion::Bbr3,
        };
        assert!(event.json().contains(r#""congestion":"bbr3""#));
    }
}
