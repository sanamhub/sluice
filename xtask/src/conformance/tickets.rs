//! The `tickets` vectors. A valid case gives the ticket text and the fields it decodes to, and the
//! lower-case text an encoder writes for them. An invalid case only has to fail.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::ticket::{Body, Format, Ticket};

use super::{Report, run_kind, shown};

#[derive(Deserialize)]
struct Case {
    ticket: String,
    valid: bool,
    /// What an encoder writes. Present when `valid` is true.
    canonical: Option<String>,
    /// What the ticket decodes to. Present when `valid` is true.
    fields: Option<Fields>,
}

/// The `fields` object. `size` and `expiry_ms` are decimal strings, because a JSON number does not
/// hold a `u64` in every language.
#[derive(Deserialize)]
struct Fields {
    version: u8,
    kind: String,
    endpoint_id: String,
    addrs: Vec<String>,
    relay_url: Option<String>,
    expiry_ms: Option<String>,
    root: Option<String>,
    size: Option<String>,
    format: Option<String>,
    stream_id: Option<String>,
    sender_key: Option<String>,
}

pub fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("tickets", dir, report, disagreement)
}

fn disagreement(case: &Case) -> Option<String> {
    let decoded = Ticket::decode(&case.ticket);
    let what = shown(&case.ticket);
    if !case.valid {
        return decoded
            .is_ok()
            .then(|| format!("{what}: expected rejected, got accepted"));
    }
    let (Some(canonical), Some(fields)) = (&case.canonical, &case.fields) else {
        return Some(format!(
            "{what}: a valid case needs `canonical` and `fields`"
        ));
    };
    let expected = match expected(fields) {
        Ok(ticket) => ticket,
        Err(reason) => return Some(format!("{what}: the fields are not a ticket: {reason}")),
    };
    match decoded {
        Err(err) => Some(format!("{what}: expected accepted, got {err}")),
        Ok(ticket) if ticket != expected => Some(format!("{what}: decoded to other fields")),
        Ok(ticket) if ticket.encode() != *canonical => {
            Some(format!("{what}: encodes to other text than `canonical`"))
        }
        Ok(_) => None,
    }
}

/// The ticket the fields describe, built without going through the decoder.
fn expected(f: &Fields) -> Result<Ticket, String> {
    if f.version != 1 {
        return Err(format!("version {}", f.version));
    }
    let addrs = f
        .addrs
        .iter()
        .map(|a| {
            a.parse::<SocketAddr>()
                .map_err(|e| format!("address {a}: {e}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expiry = f.expiry_ms.as_deref().map(number).transpose()?;
    let body = match f.kind.as_str() {
        "static" => Body::Static {
            root: hex(f.root.as_deref().ok_or("no root")?)?,
            size: number(f.size.as_deref().ok_or("no size")?)?,
            format: match f.format.as_deref() {
                Some("raw") => Format::Raw,
                Some("hash_sequence") => Format::HashSequence,
                other => return Err(format!("format {other:?}")),
            },
        },
        "live" => Body::Live {
            stream_id: hex(f.stream_id.as_deref().ok_or("no stream_id")?)?,
            sender_key: hex(f.sender_key.as_deref().ok_or("no sender_key")?)?,
        },
        other => return Err(format!("kind {other}")),
    };
    Ticket::new(
        hex(&f.endpoint_id)?,
        addrs,
        f.relay_url.clone(),
        expiry,
        body,
    )
    .map_err(|e| e.to_string())
}

fn number(text: &str) -> Result<u64, String> {
    text.parse().map_err(|e| format!("number {text}: {e}"))
}

/// A lower-case hex string as `N` bytes.
fn hex<const N: usize>(text: &str) -> Result<[u8; N], String> {
    if text.len() != 2 * N {
        return Err(format!("{text} is not {N} bytes of hex"));
    }
    let mut out = [0u8; N];
    for (byte, pair) in out.iter_mut().zip(text.as_bytes().chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|e| format!("{text}: {e}"))?;
    }
    Ok(out)
}
