//! The JSON the page speaks, in the proto3 JSON mapping with the enums as lower-case words and the
//! 64-bit counts as numbers, which the page reads. Text a peer chose is only ever a JSON string, and
//! no sentence of ours is built around it.

use serde_json::{Value, json};
use sluice_api::proto::{self, JobKind, JobState, PathKind, event::Event as Kind};

/// The word for an enum: its name without the prefix, lower case.
fn word(name: &str, prefix: &str) -> String {
    name.trim_start_matches(prefix).to_lowercase()
}

fn job_kind(value: i32) -> String {
    JobKind::try_from(value).map_or_else(
        |_| "unknown".to_string(),
        |kind| word(kind.as_str_name(), "JOB_KIND_"),
    )
}

pub fn job_state(value: i32) -> String {
    JobState::try_from(value).map_or_else(
        |_| "unknown".to_string(),
        |state| word(state.as_str_name(), "JOB_STATE_"),
    )
}

fn path_kind(value: i32) -> String {
    PathKind::try_from(value).map_or_else(
        |_| "unknown".to_string(),
        |path| word(path.as_str_name(), "PATH_KIND_"),
    )
}

fn entry_kind(value: i32) -> String {
    proto::EntryKind::try_from(value).map_or_else(
        |_| "unknown".to_string(),
        |kind| word(kind.as_str_name(), "ENTRY_KIND_"),
    )
}

fn relay(relay: &Option<proto::PeerRelayUrl>) -> Value {
    relay.as_ref().map_or(Value::Null, |url| json!(url.text))
}

pub fn version(version: &proto::VersionResponse) -> Value {
    json!({ "version": version.version, "protocol": version.protocol })
}

pub fn roots(roots: &proto::ListRootsResponse) -> Value {
    let roots: Vec<Value> = roots
        .roots
        .iter()
        .map(|r| json!({ "name": r.name }))
        .collect();
    json!({ "roots": roots })
}

pub fn entries(entries: &proto::ListEntriesResponse) -> Value {
    let listed: Vec<Value> = entries
        .entries
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name.as_ref().map_or("", |name| name.text.as_str()),
                "kind": entry_kind(entry.kind),
                "size": entry.size,
            })
        })
        .collect();
    json!({ "entries": listed, "truncated": entries.truncated })
}

pub fn job(job: &proto::JobSummary) -> Value {
    json!({
        "id": job.id,
        "kind": job_kind(job.kind),
        "state": job_state(job.state),
        "bytes_total": job.bytes_total,
        "bytes_done": job.bytes_done,
        "receivers": job.receivers,
        "resumable": job.resumable,
    })
}

pub fn jobs(jobs: &proto::ListJobsResponse) -> Value {
    json!({ "jobs": jobs.jobs.iter().map(job).collect::<Vec<_>>() })
}

pub fn inspected(offer: &proto::InspectTicketResponse) -> Value {
    json!({
        "files": offer.files,
        "folders": offer.folders,
        "links": offer.links,
        "bytes": offer.bytes,
        "names": offer.names.iter().map(|n| json!({ "text": n.text })).collect::<Vec<_>>(),
        "more_names": offer.more_names,
        "live": offer.live,
    })
}

/// One event, as one object on its own line: the job, the time, and the one thing that happened
/// under its own key.
pub fn event(event: &proto::Event) -> Value {
    let mut object = json!({ "job_id": event.job_id, "utc_ms": event.utc_ms });
    let Some(kind) = &event.event else {
        return object;
    };
    let (key, body) = match kind {
        Kind::Started(e) => (
            "started",
            json!({ "kind": job_kind(e.kind), "files": e.files, "bytes": e.bytes }),
        ),
        Kind::Connected(e) => (
            "connected",
            json!({ "path": path_kind(e.path), "relay": relay(&e.relay) }),
        ),
        Kind::PathChanged(e) => (
            "path_changed",
            json!({ "path": path_kind(e.path), "relay": relay(&e.relay) }),
        ),
        Kind::Relayed(e) => (
            "relayed",
            json!({ "relay": relay(&e.relay), "waited_seconds": e.waited_seconds }),
        ),
        Kind::Progress(e) => (
            "progress",
            json!({
                "bytes_done": e.bytes_done,
                "bytes_total": e.bytes_total,
                "bytes_per_second": e.bytes_per_second,
            }),
        ),
        Kind::Retrying(e) => (
            "retrying",
            json!({ "attempt": e.attempt, "reason": e.reason }),
        ),
        Kind::Warning(e) => (
            "warning",
            json!({
                "message": e.message,
                "name": e.name.as_ref().map(|n| n.text.as_str()),
                "relay": relay(&e.relay),
            }),
        ),
        Kind::Verified(e) => ("verified", json!({ "root": e.root, "bytes": e.bytes })),
        Kind::Done(e) => (
            "done",
            json!({
                "state": job_state(e.state),
                "message": e.message,
                "peer_close_reason": e.peer_close_reason.as_ref().map(|r| r.text.as_str()),
            }),
        ),
        Kind::Dropped(e) => ("dropped", json!({ "count": e.count })),
    };
    object[key] = body;
    object
}

/// What a path the page names is, read from a request body: `{ "root": ..., "path": ... }`.
pub fn root_path(value: &Value) -> Option<proto::RootPath> {
    Some(proto::RootPath {
        root: value.get("root")?.as_str()?.to_string(),
        path: value.get("path")?.as_str()?.to_string(),
    })
}

/// The text of a ticket in a request body: `{ "ticket": { "text": ... } }`.
pub fn ticket(value: &Value) -> Option<proto::Ticket> {
    Some(proto::Ticket {
        text: value.get("ticket")?.get("text")?.as_str()?.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done(state: JobState) -> proto::Event {
        proto::Event {
            job_id: "j".to_string(),
            utc_ms: 1,
            event: Some(Kind::Done(proto::Done {
                state: state as i32,
                message: String::new(),
                peer_close_reason: None,
            })),
        }
    }

    #[test]
    fn an_event_has_its_enums_as_lower_case_words_and_done_has_the_final_state() {
        let seen = event(&done(JobState::Cancelled));
        assert_eq!(seen["done"]["state"], "cancelled");
        let connected = event(&proto::Event {
            job_id: "j".to_string(),
            utc_ms: 1,
            event: Some(Kind::Connected(proto::Connected {
                path: proto::PathKind::Relayed as i32,
                relay: Some(proto::PeerRelayUrl {
                    text: "https://relay.example".to_string(),
                }),
            })),
        });
        assert_eq!(connected["connected"]["path"], "relayed");
        assert_eq!(connected["connected"]["relay"], "https://relay.example");
    }

    #[test]
    fn text_a_peer_chose_stays_a_json_string_whatever_it_holds() {
        let hostile = "\"; rm -rf ~ #\u{202e}.txt\n{\"done\":1}";
        let entries = entries(&proto::ListEntriesResponse {
            entries: vec![proto::DirEntry {
                name: Some(proto::PeerName {
                    text: hostile.to_string(),
                }),
                kind: proto::EntryKind::File as i32,
                size: 3,
            }],
            truncated: false,
        });
        assert_eq!(entries["entries"][0]["name"], hostile);
        // One line, however many newlines the name holds.
        assert!(!entries.to_string().contains('\n'));
    }

    #[test]
    fn a_request_value_that_is_not_the_shape_is_none() {
        assert!(root_path(&json!({ "root": "a" })).is_none());
        assert!(root_path(&json!({ "root": 1, "path": "x" })).is_none());
        assert!(ticket(&json!({ "ticket": "text" })).is_none());
        assert_eq!(
            ticket(&json!({ "ticket": { "text": "t" } })).unwrap().text,
            "t"
        );
    }
}
