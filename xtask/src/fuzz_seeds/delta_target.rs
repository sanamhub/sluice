//! The seeds of the `siglist` and `chunker` fuzz targets, read from the `delta/` vectors.

use anyhow::Context;
use serde_json::Value;

use crate::conformance::hex_bytes;

use super::text;

/// A list as the vector gives it, and the same list one byte short and one byte long, which are
/// the two ends a decoder has to refuse. A case with no list gives none.
pub(super) fn siglist(case: &Value) -> anyhow::Result<Vec<Vec<u8>>> {
    let Some(hex) = case["signatures"].as_str() else {
        return Ok(Vec::new());
    };
    let bytes = hex_bytes(hex).map_err(anyhow::Error::msg)?;
    let mut long = bytes.clone();
    long.push(0);
    Ok(vec![
        bytes[..bytes.len().saturating_sub(1)].to_vec(),
        bytes,
        long,
    ])
}

/// The candidate and the target of a case, each behind the two bytes the target reads: the average
/// as a power of two from 256, and a block to feed the stream in. An average the target cannot name
/// gives no seed.
pub(super) fn chunker(case: &Value) -> anyhow::Result<Vec<Vec<u8>>> {
    let avg = case["avg_bytes"].as_u64().context("no avg_bytes")?;
    if !(256..=8192).contains(&avg) || !avg.is_power_of_two() {
        return Ok(Vec::new());
    }
    let exponent = (avg / 256).trailing_zeros() as u8;
    let mut inputs = Vec::new();
    for key in ["candidate", "target"] {
        let data = hex_bytes(&text(case, key)?).map_err(anyhow::Error::msg)?;
        for block in [0u8, 63] {
            let mut input = vec![exponent, block];
            input.extend_from_slice(&data);
            inputs.push(input);
        }
    }
    Ok(inputs)
}
