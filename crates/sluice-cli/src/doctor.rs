//! `sluice doctor`: the local checks, one line each, with the step that fixes what is wrong.
//!
//! It only reports. The checks live in `sluice_core::doctor` and change nothing; this module says
//! what each finding means to a person and what to do about it.

use serde_json::Value;
use sluice_core::Error;
use sluice_core::doctor::{self, Check, Finding, Location, Name, Status};

use crate::exit::code_of;
use crate::output::Outcome;

#[derive(clap::Args)]
pub struct Args {}

/// One line of the report.
pub struct Line {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

/// The target triple this binary was built for, which Cargo gives only to the build script.
const TARGET: &str = env!("SLUICE_TARGET");

pub fn run(_args: &Args) -> Result<Outcome, Error> {
    let checks = doctor::examine();
    let mut lines = vec![Line {
        name: "version",
        status: Status::Ok,
        detail: format!(
            "sluice {} for {TARGET}, running on {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        ),
        fix: None,
    }];
    lines.extend(checks.iter().map(line));
    lines.push(Line {
        name: "transport",
        status: Status::Skipped,
        detail: "the transport checks come after the M0 measurements".to_string(),
        fix: None,
    });
    Ok(Outcome::Doctor {
        lines,
        code: exit_code(&checks),
    })
}

/// 0 when no check failed, since a warning does not fail. Otherwise the code of the most serious
/// kind that failed, which is the highest number of the exit code table: policy (6) over
/// storage (5).
fn exit_code(checks: &[Check]) -> u8 {
    checks
        .iter()
        .filter_map(Check::failure)
        .map(code_of)
        .max()
        .unwrap_or(0)
}

fn name(check: &Check) -> &'static str {
    match check.name() {
        Name::Home => "home",
        Name::Layout => "layout",
        Name::Lock => "lock",
        Name::Key => "key",
        Name::Space => "space",
    }
}

fn line(check: &Check) -> Line {
    let (detail, fix) = words(check.finding());
    Line {
        name: name(check),
        status: check.status(),
        detail,
        fix,
    }
}

fn words(finding: &Finding) -> (String, Option<String>) {
    use Finding as F;
    match finding {
        F::HomeReady(at) => (where_from(at), None),
        F::HomeMissing(at) => (
            format!("{} does not exist", at.path.display()),
            Some("run `sluice id`, which creates it".to_string()),
        ),
        F::HomeNotAFolder(at) => (
            format!("{} is not a folder", at.path.display()),
            Some(format!(
                "move what is at {} elsewhere, or set SLUICE_HOME to another folder",
                at.path.display()
            )),
        ),
        F::HomeFoldersMissing(at, missing) => (
            format!("{} has no {} folder", at.path.display(), missing.join(", ")),
            Some("run `sluice id`, which creates the missing folders".to_string()),
        ),
        F::HomeUnreadable(at, why) => (
            format!("{} cannot be looked at: {why}", at.path.display()),
            Some(format!(
                "check the permissions of {} and the folders above it",
                at.path.display()
            )),
        ),
        F::HomeNowhere => (
            "SLUICE_HOME is not set, and this user has no data folder".to_string(),
            Some("set SLUICE_HOME to the folder to use".to_string()),
        ),
        F::NeedsHome(_) => (
            "not checked, because the home is not there".to_string(),
            None,
        ),
        other => layout_or_lock(other).unwrap_or_else(|| key_or_space(other)),
    }
}

fn where_from(at: &Location) -> String {
    let from = if at.from_env {
        "from SLUICE_HOME"
    } else {
        "the default"
    };
    format!("{}, {from}", at.path.display())
}

fn layout_or_lock(finding: &Finding) -> Option<(String, Option<String>)> {
    use Finding as F;
    Some(match finding {
        F::LayoutCurrent(version) => (format!("version {version}"), None),
        F::LayoutOlder { found, current } => (
            format!("version {found}, older than the {current} this sluice writes"),
            Some(format!(
                "run `sluice id`, which migrates it to version {current}"
            )),
        ),
        F::LayoutNoFile(current) => (
            "no VERSION file".to_string(),
            Some(format!("run `sluice id`, which writes version {current}")),
        ),
        F::LayoutNewer { found, known } => (
            format!("version {found}, newer than the {known} this sluice reads"),
            Some(
                "use the newer sluice that wrote it, or set SLUICE_HOME to another folder"
                    .to_string(),
            ),
        ),
        F::LayoutUnreadable(why) => (
            why.clone(),
            Some(
                "make VERSION hold the layout version and a newline: 1 for a home this sluice made"
                    .to_string(),
            ),
        ),
        F::LockFree => ("free".to_string(), None),
        F::LockHeld(pid) => (format!("held by a running sluice{}", pid_text(*pid)), None),
        F::LockStale(pid) => (
            format!(
                "free, and lock.pid is left by a sluice{} that did not exit cleanly",
                pid_text(*pid)
            ),
            Some("run `sluice id`, which reclaims it".to_string()),
        ),
        F::LockUnreadable(why) => (
            format!("cannot be tried: {why}"),
            Some("check the permissions of the lock file in the home".to_string()),
        ),
        _ => return None,
    })
}

fn pid_text(pid: Option<u32>) -> String {
    pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default()
}

fn key_or_space(finding: &Finding) -> (String, Option<String>) {
    use Finding as F;
    match finding {
        F::KeyPrivate => ("32 bytes, readable only by this user".to_string(), None),
        F::KeyMissing => (
            "no identity key yet".to_string(),
            Some("run `sluice id`, which creates it".to_string()),
        ),
        F::KeyWrongSize(size) => (
            format!("{size} bytes, and an identity key is 32"),
            Some(
                "restore identity.key from a backup, or move it elsewhere and run `sluice id` \
                 for a new identity"
                    .to_string(),
            ),
        ),
        F::KeyOpenToOthers(mode) => (
            format!("mode {mode:04o}, so other users may be able to read it"),
            Some("run `chmod 600` on identity.key in the home".to_string()),
        ),
        F::HomeOpenToOthers(mode) => (
            format!("the home has mode {mode:04o}, so other users can reach the key"),
            Some("run `chmod 700` on the home".to_string()),
        ),
        F::KeyAclNotPrivate => (
            "its access list lets in more than this user, SYSTEM and Administrators".to_string(),
            Some(
                "in the Properties of identity.key, under Security > Advanced, disable \
                 inheritance and remove every entry but yours, SYSTEM and Administrators"
                    .to_string(),
            ),
        ),
        F::KeyUnreadable(why) => (
            format!("cannot be checked: {why}"),
            Some("check the permissions of identity.key in the home".to_string()),
        ),
        F::SpaceFree(free) => space(*free),
        F::SpaceUnreadable(why) => (
            format!("cannot be read: {why}"),
            Some("check that the home is on a mounted volume".to_string()),
        ),
        _ => (String::new(), None),
    }
}

fn space(free: u64) -> (String, Option<String>) {
    let gib = free as f64 / f64::from(1u32 << 30);
    let detail = format!("{gib:.1} GiB free ({free} bytes)");
    if free < doctor::LOW_SPACE_BYTES {
        let fix =
            "free space on the home's volume: under 2 GiB, a transfer can run out".to_string();
        (detail, Some(fix))
    } else {
        (detail, None)
    }
}

/// The report as a person reads it: a line per check, and the fix under the line it is for. With
/// colour, only the status word is coloured, so the columns line up either way.
pub fn human(lines: &[Line], colour: bool) -> String {
    let mut out = String::new();
    for line in lines {
        let word = status_word(line.status);
        let pad = " ".repeat(8 - word.len());
        let shown = if colour {
            format!("{}{word}\x1b[0m{pad}", paint(line.status))
        } else {
            format!("{word}{pad}")
        };
        out.push_str(&format!("{shown} {:<10} {}\n", line.name, line.detail));
        if let Some(fix) = &line.fix {
            out.push_str(&format!("{:20}fix: {fix}\n", ""));
        }
    }
    out
}

/// The escape code that starts the colour of a status.
fn paint(status: Status) -> &'static str {
    match status {
        Status::Ok => "\x1b[32m",
        Status::Warning => "\x1b[33m",
        Status::Failed => "\x1b[1;31m",
        Status::Skipped => "\x1b[2m",
    }
}

/// The report as one JSON object: `{"checks": [{"name", "status", "detail", "fix"}]}`, with `fix`
/// null when there is nothing to do. Each object is put together here so that its keys come in
/// that order, which `json!` would sort. `serde_json` writes every value, so a quote or a control
/// character in a path is escaped.
pub fn json(lines: &[Line]) -> String {
    let checks: Vec<String> = lines
        .iter()
        .map(|line| {
            format!(
                "{{\"name\":{},\"status\":{},\"detail\":{},\"fix\":{}}}",
                Value::from(line.name),
                Value::from(status_word(line.status)),
                Value::from(line.detail.as_str()),
                line.fix.as_deref().map_or(Value::Null, Value::from),
            )
        })
        .collect();
    format!("{{\"checks\":[{}]}}\n", checks.join(","))
}

/// The word for a status, which is also its JSON value.
fn status_word(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Warning => "warning",
        Status::Failed => "failed",
        Status::Skipped => "skipped",
    }
}
