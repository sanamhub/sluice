//! The `writer` vectors: the top-level rules a receive applies to a manifest path before it fetches
//! any content. Each case is a name, what is already in the output folder, the flags the caller
//! set, and the one answer the rules give.

use std::path::Path;

use anyhow::Result;
use serde::Deserialize;
use sluice_core::engine::top_level_rule;

use super::{Report, run_kind};

#[derive(Deserialize)]
struct Case {
    name: String,
    /// Something already stands at this top-level name in the output folder.
    exists: bool,
    allow_hidden: bool,
    replace: bool,
    resume: bool,
    /// `accept`, `hidden` or `clash`.
    expect: String,
}

pub(crate) fn run(dir: &Path, report: &mut Report) -> Result<()> {
    run_kind("writer", dir, report, |case: &Case| {
        let got = top_level_rule(
            &case.name,
            case.exists,
            case.allow_hidden,
            case.replace,
            case.resume,
        );
        let agrees = matches!(
            (case.expect.as_str(), got),
            ("accept", Ok(())) | ("hidden", Err("hidden")) | ("clash", Err("clash"))
        );
        if agrees {
            None
        } else {
            Some(format!(
                "{}: expected {}, got {:?}",
                case.name, case.expect, got
            ))
        }
    })
}
