//! The event values a transfer reports, and the one rule that turns a path into the line it prints.

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
