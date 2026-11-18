use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;

use anyhow::{Result, anyhow};
use serde_json::Value;
use sluice_meet::frame::{ClientFrame, ErrorCode, FailReason, Role, ServerFrame};
use sluice_meet::meet::{Config, ConnectionId, MeetingPlace, Outbound};

use super::{Report, hex_bytes};

pub(super) fn run(dir: &Path, report: &mut Report) -> Result<()> {
    let path = dir.join("meeting").join("meeting.json");
    let text = std::fs::read_to_string(&path)
        .map_err(|error| anyhow!("cannot read {}: {error}", path.display()))?;
    let file: Value = serde_json::from_str(&text)
        .map_err(|error| anyhow!("{} is not a vector file: {error}", path.display()))?;
    let description = file["description"].as_str().unwrap_or_default();
    let cases = file["cases"]
        .as_array()
        .ok_or_else(|| anyhow!("meeting.json has no cases"))?;
    let failures: Vec<String> = cases
        .iter()
        .filter_map(meeting_case)
        .map(|what| format!("FAIL {what}. {description}"))
        .collect();
    report.lines.push(format!(
        "short-code/meeting/meeting.json  {} of {} passed",
        cases.len() - failures.len(),
        cases.len()
    ));
    report
        .lines
        .extend(failures.iter().map(|line| format!("  {line}")));
    report.passed.push((
        "short-code/meeting".to_string(),
        cases.len() - failures.len(),
    ));
    report.failed += failures.len();
    Ok(())
}

fn meeting_case(case: &Value) -> Option<String> {
    let name = case["name"].as_str().unwrap_or("a case");
    let config = &case["config"];
    let Some(config) = config_u32(config, "rate_per_ip")
        .zip(config_u32(config, "max_plates"))
        .zip(config_u32(config, "ttl_seconds"))
        .map(|((rate, plates), ttl)| Config::new(rate, plates as usize, u64::from(ttl)))
    else {
        return Some(format!("{name}: the config is not the three numbers"));
    };
    let Ok(config) = config else {
        return Some(format!("{name}: the config is outside the ADR's bounds"));
    };
    let mut place = MeetingPlace::new(config);
    let mut labels: HashMap<String, ConnectionId> = HashMap::new();
    let Some(steps) = case["steps"].as_array() else {
        return Some(format!("{name}: no steps"));
    };
    for step in steps {
        if let Some(label) = step["connect"].as_str() {
            let ip: IpAddr = match step["ip"].as_str().and_then(|ip| ip.parse().ok()) {
                Some(ip) => ip,
                None => return Some(format!("{name}: a connection has no IP")),
            };
            match place.connect(ip, step["now_ms"].as_u64().unwrap_or(0)) {
                Ok(id) => {
                    labels.insert(label.to_string(), id);
                }
                Err(error) => return Some(format!("{name}: connect refused as {error}")),
            }
            continue;
        }
        if let Some(label) = step["disconnect"].as_str() {
            let Some(id) = labels.get(label).copied() else {
                return Some(format!("{name}: no connection {label:?}"));
            };
            let got = place.disconnect(id, step["now_ms"].as_u64().unwrap_or(0));
            if let Some(what) = mismatch(&got, &step["out"], &labels) {
                return Some(format!("{name}: {what}"));
            }
            continue;
        }
        if let Some(now) = step["expire"].as_u64() {
            let got = place.expire(now);
            if let Some(what) = mismatch(&got, &step["out"], &labels) {
                return Some(format!("{name}: {what}"));
            }
            continue;
        }
        let Some(label) = step["receive"].as_str() else {
            return Some(format!("{name}: a step names no operation"));
        };
        let Some(id) = labels.get(label).copied() else {
            return Some(format!("{name}: no connection {label:?}"));
        };
        let frame = match client_frame(step) {
            Ok(frame) => frame,
            Err(what) => return Some(format!("{name}: {what}")),
        };
        let now = step["now_ms"].as_u64().unwrap_or(0);
        let got = place.receive(id, frame, now);
        if let Some(error) = step["expect_error"].as_str() {
            return match got {
                Err(code) if error_name(code) == error => None,
                Err(code) => Some(format!(
                    "{name}: expected {error}, got {}",
                    error_name(code)
                )),
                Ok(_) => Some(format!("{name}: expected {error}, got accepted")),
            };
        }
        let got = match got {
            Ok(out) => out,
            Err(code) => return Some(format!("{name}: refused as {}", error_name(code))),
        };
        if let Some(what) = mismatch(&got, &step["out"], &labels) {
            return Some(format!("{name}: {what}"));
        }
    }
    match case["plates"].as_u64() {
        Some(want) if want as usize != place.plates() => Some(format!(
            "{name}: {} plates, expected {want}",
            place.plates()
        )),
        _ => None,
    }
}

fn config_u32(config: &Value, name: &str) -> Option<u32> {
    u32::try_from(config[name].as_u64()?).ok()
}

fn client_frame(step: &Value) -> Result<ClientFrame, String> {
    match step["frame"].as_str().unwrap_or_default() {
        "claim" => Ok(ClientFrame::Claim),
        "join" => Ok(ClientFrame::Join(
            u32::try_from(step["nameplate"].as_u64().unwrap_or(0))
                .map_err(|_| "a nameplate is too large".to_string())?,
        )),
        "spake" => Ok(ClientFrame::Spake(hex_bytes(
            step["hex"].as_str().unwrap_or(""),
        )?)),
        "confirm" => Ok(ClientFrame::Confirm(
            hex_bytes(step["hex"].as_str().unwrap_or(""))?
                .try_into()
                .map_err(|_| "a confirmation is not 32 bytes".to_string())?,
        )),
        "sealed" => Ok(ClientFrame::Sealed(hex_bytes(
            step["hex"].as_str().unwrap_or(""),
        )?)),
        "fail" => Ok(ClientFrame::Fail(match step["reason"].as_str() {
            Some("confirm") => FailReason::ConfirmMismatch,
            Some("seal") => FailReason::SealFailed,
            other => return Err(format!("a fail reason is {other:?}")),
        })),
        "done" => Ok(ClientFrame::Done),
        other => Err(format!("unknown client frame {other:?}")),
    }
}

fn mismatch(
    got: &[Outbound],
    want: &Value,
    labels: &HashMap<String, ConnectionId>,
) -> Option<String> {
    if want.is_null() {
        // A setup step checks only that it was accepted; a step that matters says `out`, even when
        // that is `[]`.
        return None;
    }
    let Some(want) = want.as_array() else {
        return Some("`out` is not an array".to_string());
    };
    if got.len() != want.len() {
        return Some(format!("{} frames, expected {}", got.len(), want.len()));
    }
    for (at, (got, want)) in got.iter().zip(want).enumerate() {
        let Some(label) = want["to"].as_str() else {
            return Some(format!("frame {at} names no connection"));
        };
        let Some(id) = labels.get(label).copied() else {
            return Some(format!("frame {at} names no connection {label:?}"));
        };
        if got.to != id {
            return Some(format!("frame {at} went to another connection"));
        }
        let frame = match server_frame(want) {
            Ok(frame) => frame,
            Err(what) => return Some(format!("frame {at}: {what}")),
        };
        if got.frame != frame {
            return Some(format!("frame {at} is {:?}, expected {frame:?}", got.frame));
        }
    }
    None
}

fn server_frame(value: &Value) -> Result<ServerFrame, String> {
    match value["frame"].as_str().unwrap_or_default() {
        "claimed" => Ok(ServerFrame::Claimed(
            u32::try_from(value["nameplate"].as_u64().unwrap_or(0))
                .map_err(|_| "a nameplate is too large".to_string())?,
        )),
        "joined" => Ok(ServerFrame::Joined(match value["role"].as_str() {
            Some("sender") => Role::Sender,
            Some("receiver") => Role::Receiver,
            other => return Err(format!("a role is {other:?}")),
        })),
        "burned" => Ok(ServerFrame::Burned),
        "peer_closed" => Ok(ServerFrame::PeerClosed),
        "done" => Ok(ServerFrame::Done),
        "error" => Ok(ServerFrame::Error(match value["code"].as_str() {
            Some("no_code") => ErrorCode::NoCode,
            Some("busy") => ErrorCode::Busy,
            Some("full") => ErrorCode::Full,
            Some("expired") => ErrorCode::Expired,
            Some("too_large") => ErrorCode::TooLarge,
            Some("bad_frame") => ErrorCode::BadFrame,
            Some("rate_limited") => ErrorCode::RateLimited,
            other => return Err(format!("an error code is {other:?}")),
        })),
        other => Err(format!("unknown server frame {other:?}")),
    }
}

fn error_name(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::NoCode => "no_code",
        ErrorCode::Busy => "busy",
        ErrorCode::Full => "full",
        ErrorCode::Expired => "expired",
        ErrorCode::TooLarge => "too_large",
        ErrorCode::BadFrame => "bad_frame",
        ErrorCode::RateLimited => "rate_limited",
    }
}
