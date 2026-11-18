//! The `short-code` vectors: the code, the word list, the transcript and key schedule, the sealed
//! ticket, every frame, and a scripted meeting place.

use std::path::Path;

use anyhow::{Result, anyhow};
use serde::Deserialize;
use sluice_meet::code::{Code, CodeError};
use sluice_meet::crypto::{Keys, transcript};
use sluice_meet::frame::{ClientFrame, FrameError, Role, ServerFrame};
use sluice_meet::words;

use super::{Report, hex_bytes, run_kind, shown};

mod meeting;

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    let reference = dir.join("words.txt");
    match std::fs::read_to_string(&reference) {
        Ok(text) if text == words::TEXT && words::digest_hex() == words::SHA256 => {
            report
                .lines
                .push("short-code/words.txt  2048 words, the pinned SHA-256".to_string());
            report.passed.push(("short-code/words".to_string(), 1));
        }
        Ok(_) => {
            report.lines.push(
                "short-code/words.txt  FAIL the reference copy differs from the crate's list"
                    .to_string(),
            );
            report.failed += 1;
        }
        Err(error) => {
            report
                .lines
                .push(format!("short-code/words.txt  FAIL cannot read: {error}"));
            report.failed += 1;
        }
    }
    run_kind("short-code/codes", &dir.join("codes"), report, code_case)?;
    run_kind(
        "short-code/transcript",
        &dir.join("transcript"),
        report,
        transcript_case,
    )?;
    run_kind("short-code/keys", &dir.join("keys"), report, keys_case)?;
    run_kind(
        "short-code/sealed",
        &dir.join("sealed"),
        report,
        sealed_case,
    )?;
    frames(dir, report)?;
    meeting::run(dir, report)?;
    Ok(())
}

#[derive(Deserialize)]
struct CodeCase {
    text: String,
    #[serde(default)]
    nameplate: Option<u32>,
    #[serde(default)]
    words: Option<Vec<u16>>,
    #[serde(default)]
    rule: Option<String>,
}

fn code_case(case: &CodeCase) -> Option<String> {
    let what = shown(&case.text);
    let parsed = Code::parse(&case.text);
    if let Some(rule) = &case.rule {
        return match parsed {
            Ok(_) => Some(format!("{what}: expected {rule}, got accepted")),
            Err(error) if rule_name(error) == rule => None,
            Err(error) => Some(format!("{what}: expected {rule}, got {}", rule_name(error))),
        };
    }
    match parsed {
        Err(error) => Some(format!(
            "{what}: expected accepted, got {}",
            rule_name(error)
        )),
        Ok(code) => {
            if Some(code.nameplate()) != case.nameplate {
                return Some(format!("{what}: another nameplate"));
            }
            if case
                .words
                .as_deref()
                .is_none_or(|words| words != code.word_indices())
            {
                return Some(format!("{what}: another word pair"));
            }
            if code.text() != case.text.to_ascii_lowercase() {
                return Some(format!("{what}: prints as another spelling"));
            }
            None
        }
    }
}

fn rule_name(error: CodeError) -> &'static str {
    match error {
        CodeError::Shape => "shape",
        CodeError::Nameplate => "nameplate",
        CodeError::Word => "word",
    }
}

#[derive(Deserialize)]
struct TranscriptCase {
    nameplate: u32,
    a: String,
    b: String,
    transcript: String,
}

fn transcript_case(case: &TranscriptCase) -> Option<String> {
    let (Ok(a), Ok(b), Ok(want)) = (
        hex_bytes(&case.a),
        hex_bytes(&case.b),
        hex_bytes(&case.transcript),
    ) else {
        return Some("a transcript case has a hex field that is not hex".to_string());
    };
    let got = transcript(case.nameplate, &a, &b);
    (got != want).then(|| format!("nameplate {}: another transcript", case.nameplate))
}

#[derive(Deserialize)]
struct KeysCase {
    shared: String,
    transcript: String,
    confirm_sender: String,
    confirm_receiver: String,
    ticket: String,
}

fn keys_case(case: &KeysCase) -> Option<String> {
    let (Ok(shared), Ok(transcript), Ok(sender), Ok(receiver), Ok(ticket)) = (
        hex_bytes(&case.shared),
        hex_bytes(&case.transcript),
        hex_bytes(&case.confirm_sender),
        hex_bytes(&case.confirm_receiver),
        hex_bytes(&case.ticket),
    ) else {
        return Some("a keys case has a hex field that is not hex".to_string());
    };
    let keys = match Keys::from_shared(&shared, &transcript) {
        Ok(keys) => keys,
        Err(error) => return Some(format!("the keys did not derive: {error}")),
    };
    if keys.confirmation(Role::Sender).as_slice() != sender {
        return Some("another sender confirmation tag".to_string());
    }
    if keys.confirmation(Role::Receiver).as_slice() != receiver {
        return Some("another receiver confirmation tag".to_string());
    }
    if keys.ticket_key().as_slice() != ticket {
        return Some("another ticket key".to_string());
    }
    None
}

#[derive(Deserialize)]
struct SealedCase {
    shared: String,
    transcript: String,
    nonce: String,
    ticket: String,
    sealed: String,
    opens: bool,
}

fn sealed_case(case: &SealedCase) -> Option<String> {
    let (Ok(shared), Ok(transcript), Ok(nonce), Ok(sealed)) = (
        hex_bytes(&case.shared),
        hex_bytes(&case.transcript),
        hex_bytes(&case.nonce),
        hex_bytes(&case.sealed),
    ) else {
        return Some("a sealed case has a hex field that is not hex".to_string());
    };
    let Ok(nonce): Result<[u8; 12], _> = nonce.try_into() else {
        return Some("a nonce is not 12 bytes".to_string());
    };
    let keys = match Keys::from_shared(&shared, &transcript) {
        Ok(keys) => keys,
        Err(error) => return Some(format!("the keys did not derive: {error}")),
    };
    if case.opens {
        match keys.seal_with_nonce(&nonce, &case.ticket) {
            Ok(got) if got == sealed => {}
            Ok(_) => return Some("the sealed bytes differ".to_string()),
            Err(error) => return Some(format!("the ticket did not seal: {error}")),
        }
        match keys.open(&sealed) {
            Ok(opened) if opened.as_str() == case.ticket => None,
            Ok(_) => Some("the opened ticket differs".to_string()),
            Err(error) => Some(format!("a valid seal did not open: {error}")),
        }
    } else if keys.open(&sealed).is_ok() {
        Some("a wrong-tag seal opened".to_string())
    } else {
        None
    }
}

#[derive(Deserialize)]
struct FrameCase {
    #[serde(default)]
    frame: Option<String>,
    hex: String,
    #[serde(default)]
    text: bool,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

fn frames(dir: &Path, report: &mut Report) -> Result<()> {
    for name in ["client", "server", "invalid"] {
        let path = dir.join("frames").join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path)
            .map_err(|error| anyhow!("cannot read {}: {error}", path.display()))?;
        let file: File = serde_json::from_str(&text)
            .map_err(|error| anyhow!("{} is not a vector file: {error}", path.display()))?;
        let failures: Vec<String> = file
            .cases
            .iter()
            .filter_map(|case| frame_case(case, name))
            .map(|what| format!("FAIL {what}. {}", file.description))
            .collect();
        report.lines.push(format!(
            "short-code/frames/{name}.json  {} of {} passed",
            file.cases.len() - failures.len(),
            file.cases.len()
        ));
        report
            .lines
            .extend(failures.iter().map(|line| format!("  {line}")));
        report.passed.push((
            "short-code/frames".to_string(),
            file.cases.len() - failures.len(),
        ));
        report.failed += failures.len();
    }
    Ok(())
}

fn frame_case(case: &FrameCase, file: &str) -> Option<String> {
    let Ok(bytes) = hex_bytes(&case.hex) else {
        return Some("a frame is not hex".to_string());
    };
    let direction = case.direction.as_deref().unwrap_or(file);
    if let Some(error) = &case.error {
        let got = match direction {
            "client" => ClientFrame::decode_message(case.text, &bytes).err(),
            "server" => ServerFrame::decode_message(case.text, &bytes).err(),
            _ => return Some(format!("unknown direction {direction:?}")),
        };
        return match got {
            Some(got) if frame_error(got) == error => None,
            Some(got) => Some(format!("expected {error}, got {}", frame_error(got))),
            None => Some(format!("expected {error}, got accepted")),
        };
    }
    let Some(frame) = &case.frame else {
        return Some("a valid case needs `frame` or `error`".to_string());
    };
    match direction {
        "client" => {
            let Ok(decoded) = ClientFrame::decode(&bytes) else {
                return Some(format!("{frame}: could not decode"));
            };
            match decoded.encode() {
                Ok(again) if again == bytes => None,
                Ok(_) => Some(format!("{frame}: encoded other bytes")),
                Err(error) => Some(format!("{frame}: did not encode: {error}")),
            }
        }
        "server" => {
            let Ok(decoded) = ServerFrame::decode(&bytes) else {
                return Some(format!("{frame}: could not decode"));
            };
            match decoded.encode() {
                Ok(again) if again == bytes => None,
                Ok(_) => Some(format!("{frame}: encoded other bytes")),
                Err(error) => Some(format!("{frame}: did not encode: {error}")),
            }
        }
        _ => Some(format!("unknown direction {direction:?}")),
    }
}

fn frame_error(error: FrameError) -> &'static str {
    match error {
        FrameError::Text => "text",
        FrameError::TooLarge => "too_large",
        FrameError::Truncated => "truncated",
        FrameError::Unknown => "unknown",
        FrameError::Trailing => "trailing",
        FrameError::BadRole => "bad_role",
        FrameError::BadReason => "bad_reason",
        FrameError::BadCode => "bad_code",
        FrameError::BadSealed => "bad_sealed",
    }
}

#[derive(Deserialize)]
struct File {
    description: String,
    cases: Vec<FrameCase>,
}
