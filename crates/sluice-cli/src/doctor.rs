//! `sluice doctor`: the local checks, one line each, with the step that fixes what is wrong.
//!
//! It only reports. The checks live in `sluice_core::doctor` and change nothing; this module says
//! what each finding means to a person and what to do about it.

use serde_json::Value;
use sluice_core::Error;
use sluice_core::doctor::{self, Check, Name, Status};
use sluice_core::engine::RelayModeChoice;

mod words;

use crate::exit::code_of;
use crate::output::Outcome;
use words::words;

#[derive(clap::Args)]
pub struct Args {
    /// Also check the network: whether UDP can be bound, the relay, the public address, the NAT
    /// mapping and whether a router can open a port. With a relay that relay is contacted; without
    /// one two checks stay on the local network, an mDNS query on every interface and a
    /// port-mapping request to the router. The plain report stays local and quick
    #[arg(long)]
    pub network: bool,
    /// The relay the network checks use, and the one a short code would meet at
    #[arg(long, value_name = "URL")]
    pub relay: Option<String>,
    /// Use n0's public relays for the network checks (not run by this project)
    #[arg(long, requires = "network")]
    pub n0_relays: bool,
}

/// One line of the report.
pub struct Line {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

/// The target triple this binary was built for, which Cargo gives only to the build script.
const TARGET: &str = env!("SLUICE_TARGET");

pub fn run(args: &Args) -> Result<Outcome, Error> {
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
    lines.push(meet(args)?);
    if args.network {
        let relay = RelayModeChoice::from_flags(args.relay.as_deref(), args.n0_relays)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Storage(format!("cannot start the check runtime: {e}")))?;
        lines.extend(runtime.block_on(crate::network::lines(&relay)));
    } else {
        lines.push(Line {
            name: "transport",
            status: Status::Skipped,
            detail:
                "pass --network to check UDP, the relay, the public address, NAT and port mapping"
                    .to_string(),
            fix: None,
        });
    }
    Ok(Outcome::Doctor {
        lines,
        code: exit_code(&checks),
    })
}

/// The meeting place a code would use: the configured relay's `/meet`, and whether that relay
/// answers there. It never makes a code; one upgrade and a close are enough.
fn meet(args: &Args) -> Result<Line, Error> {
    let Some(relay) = args.relay.as_deref() else {
        return Ok(Line {
            name: "meet",
            status: Status::Skipped,
            detail: "no relay is configured; a short code needs a relay with a meeting place"
                .to_string(),
            fix: Some("pass --relay URL on both sides, or send without --code".to_string()),
        });
    };
    let url = crate::short_code::meet_url(relay)?;
    let detail =
        format!("{url} on the relay; a code only works when both sides name the same relay");
    if !args.network {
        return Ok(Line {
            name: "meet",
            status: Status::Skipped,
            detail,
            fix: Some("pass --network to check whether the relay serves it".to_string()),
        });
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Storage(format!("cannot start the check runtime: {e}")))?;
    match runtime.block_on(crate::short_code::serves(relay)) {
        Ok(()) => Ok(Line {
            name: "meet",
            status: Status::Ok,
            detail,
            fix: None,
        }),
        Err(error) => Ok(Line {
            name: "meet",
            status: Status::Warning,
            detail: format!("{detail}; {error}"),
            fix: Some("add [meet] to the relay, or send without --code".to_string()),
        }),
    }
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
        Name::Access => "access",
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
