//! The `log` fuzz target's seeds: the records after a case's open, in the target's framing.

use anyhow::{Context, bail};
use serde_json::Value;

use crate::conformance::hex_bytes;

use super::{framing, text};

/// The records after the case's open, in the target's framing, up to a second open, which the
/// framing has no tag for. The target opens its stream itself, with `framing::OPEN`, so only a
/// case that starts on those bytes gives a seed. Any other open has another stream, key or segment
/// size, and the target would read its records against the wrong one, so the case is skipped and
/// gives `None`. A case that does not start on an open, or a record longer than the framing takes,
/// is an error naming the case, and never a seed that says something else.
pub(super) fn log(case: &Value) -> anyhow::Result<Option<Vec<u8>>> {
    let name = text(case, "name")?;
    let steps = case["steps"].as_array().context("no steps")?;
    let first = steps.first().map(|step| text(step, "record")).transpose()?;
    if first.as_deref() != Some("open") {
        bail!("case {name:?} does not start with an open, which the log target takes as given");
    }
    if hex_bytes(&text(&steps[0], "bytes")?).map_err(anyhow::Error::msg)? != framing::OPEN {
        return Ok(None);
    }
    let mut input = Vec::new();
    for step in &steps[1..] {
        let record = text(step, "record")?;
        if record == "open" {
            break;
        }
        let bytes = hex_bytes(&text(step, "bytes")?).map_err(anyhow::Error::msg)?;
        framing::push(&mut input, record == "close", &bytes).map_err(|len| {
            anyhow::anyhow!(
                "case {name:?} has a {record} of {len} bytes, and the log target's records are \
                 at most {} bytes",
                framing::MAX_RECORD_BYTES
            )
        })?;
    }
    Ok(Some(input))
}
