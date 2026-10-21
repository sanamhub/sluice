//! The `live` vectors. A case is a ticket's `stream_id` and `sender_key` and the records a
//! receiver is given, in order. Each step says what the receiver answers, and the case says
//! where the stream stands at the end.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_live::{Error, Fault, Outcome, Receiver, Status};

use super::manifests::{hex_array, hex_bytes, number};
use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    stream_id: String,
    sender_key: String,
    steps: Vec<Step>,
    #[serde(rename = "final")]
    end: Expected,
}

#[derive(Deserialize)]
struct Step {
    /// `open`, `frame` or `close`.
    record: String,
    bytes: String,
    #[serde(flatten)]
    expected: Expected,
}

/// An outcome, or the state at the end. The field that names it is `outcome` in a step and
/// `state` at the end; the other fields are the same.
#[derive(Deserialize)]
struct Expected {
    #[serde(alias = "state")]
    outcome: String,
    rule: Option<u8>,
    seq: Option<String>,
    next_seq: Option<String>,
    segment_root: Option<String>,
    len: Option<String>,
    final_root: Option<String>,
    total_len: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("live", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let what = format!("{:?}", case.name);
    check(case).err().map(|reason| format!("{what}: {reason}"))
}

fn check(case: &Case) -> Result<(), String> {
    let stream_id: [u8; 16] = hex_bytes(&case.stream_id)?
        .try_into()
        .map_err(|_| "stream_id is not 16 bytes".to_string())?;
    let sender_key = hex_array(&case.sender_key)?;
    let mut receiver: Option<Receiver> = None;
    for (at, step) in case.steps.iter().enumerate() {
        let bytes = hex_bytes(&step.bytes)?;
        let got = match (step.record.as_str(), receiver.as_mut()) {
            ("open", None) => match Receiver::new(stream_id, sender_key, &bytes) {
                Ok(started) => {
                    receiver = Some(started);
                    "started".to_string()
                }
                Err(Error::Verify(fault) | Error::Policy(fault)) => refused(fault),
            },
            ("frame", Some(r)) => outcome(r.frame(&bytes)),
            ("close", Some(r)) => outcome(r.close(&bytes)),
            (record, _) => return Err(format!("step {at}: a {record} step out of place")),
        };
        let want = expected(&step.expected)?;
        if got != want {
            return Err(format!(
                "step {at} ({}): got {got}, want {want}",
                step.record
            ));
        }
    }
    let got = receiver.map_or("not started".to_string(), |r| status(r.status()));
    let want = expected(&case.end)?;
    if got != want {
        return Err(format!("at the end: got {got}, want {want}"));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn refused(fault: Fault) -> String {
    format!("refused rule {}", fault.rule())
}

fn outcome(outcome: Outcome) -> String {
    match outcome {
        Outcome::Accepted {
            seq,
            segment_root,
            len,
        } => format!("accepted seq {seq} root {} len {len}", hex(&segment_root)),
        Outcome::Repeat { seq } => format!("repeat seq {seq}"),
        Outcome::Refused(fault) => refused(fault),
        Outcome::Closed {
            final_root,
            total_len,
        } => format!("closed {} {total_len}", hex(&final_root)),
        Outcome::Aborted { seq, reason } => format!("aborted seq {seq} rule {}", reason.rule()),
    }
}

fn status(status: Status) -> String {
    match status {
        Status::Open { next_seq } => format!("open next {next_seq}"),
        Status::Closed {
            final_root,
            total_len,
        } => format!("closed {} {total_len}", hex(&final_root)),
        Status::Aborted { seq, reason } => format!("aborted seq {seq} rule {}", reason.rule()),
    }
}

/// The vector's outcome in the words `outcome` and `status` use, so the two compare as text and
/// a failure line shows both.
fn expected(e: &Expected) -> Result<String, String> {
    let text = |field: &Option<String>, name: &str| {
        field
            .clone()
            .ok_or_else(|| format!("{} with no {name}", e.outcome))
    };
    let num = |field: &Option<String>, name: &str| text(field, name).and_then(|t| number(&t));
    let rule = || e.rule.ok_or_else(|| format!("{} with no rule", e.outcome));
    Ok(match e.outcome.as_str() {
        "started" | "not started" => e.outcome.clone(),
        "accepted" => format!(
            "accepted seq {} root {} len {}",
            num(&e.seq, "seq")?,
            text(&e.segment_root, "segment_root")?,
            num(&e.len, "len")?
        ),
        "repeat" => format!("repeat seq {}", num(&e.seq, "seq")?),
        "refused" => format!("refused rule {}", rule()?),
        "open" => format!("open next {}", num(&e.next_seq, "next_seq")?),
        "closed" => format!(
            "closed {} {}",
            text(&e.final_root, "final_root")?,
            num(&e.total_len, "total_len")?
        ),
        "aborted" => format!("aborted seq {} rule {}", num(&e.seq, "seq")?, rule()?),
        other => return Err(format!("an outcome {other:?}")),
    })
}
