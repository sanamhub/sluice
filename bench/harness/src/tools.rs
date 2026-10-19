//! What a host has installed. The harness reports what is missing and never installs anything.

use std::time::Duration;

use anyhow::Result;
use serde::Serialize;

use crate::host::{Host, Os, quote};

const QUICK: Duration = Duration::from_secs(30);

/// The tools a run can use besides the probe.
pub const OPTIONAL: [&str; 4] = ["iperf3", "fio", "tc", "mscp"];

/// One tool on one host.
#[derive(Clone, Debug, Serialize)]
pub struct Tool {
    /// Command name.
    pub name: String,
    /// True if the host can run it.
    pub found: bool,
    /// First line of `--version`, if it printed one.
    pub version: Option<String>,
}

impl Tool {
    fn missing(name: &str) -> Self {
        Self {
            name: name.to_string(),
            found: false,
            version: None,
        }
    }
}

/// The probe and the optional tools on `host`.
pub async fn detect(host: &Host) -> Result<Vec<Tool>> {
    let mut tools = vec![probe(host).await?];
    for name in OPTIONAL {
        tools.push(optional(host, name).await?);
    }
    Ok(tools)
}

/// The tool named `name` in `tools`, if it was found.
pub fn found<'a>(tools: &'a [Tool], name: &str) -> Option<&'a Tool> {
    tools.iter().find(|t| t.name == name && t.found)
}

async fn probe(host: &Host) -> Result<Tool> {
    let out = host.run(&[&host.entry.probe, "--version"], QUICK).await;
    Ok(match out {
        Ok(out) if out.success() => Tool {
            name: "probe".to_string(),
            found: true,
            version: first_line(&out.stdout),
        },
        _ => Tool::missing("probe"),
    })
}

async fn optional(host: &Host, name: &str) -> Result<Tool> {
    let on_path = if host.os == Os::Windows {
        host.run(&["where", name], QUICK).await
    } else {
        let script = format!("command -v {}", quote(name));
        host.run(&["sh", "-c", &script], QUICK).await
    };
    if !on_path.is_ok_and(|o| o.success()) {
        return Ok(Tool::missing(name));
    }
    // `tc` has no --version, and prints its usage for it.
    let version = if name == "tc" {
        None
    } else {
        let out = host.run(&[name, "--version"], QUICK).await;
        out.ok()
            .and_then(|o| first_line(&o.stdout).or_else(|| first_line(&o.stderr)))
    };
    Ok(Tool {
        name: name.to_string(),
        found: true,
        version,
    })
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_non_empty_line_is_the_version() {
        assert_eq!(
            first_line("\n iperf 3.16 (cJSON 1.7.15)\nLinux\n"),
            Some("iperf 3.16 (cJSON 1.7.15)".to_string())
        );
        assert_eq!(first_line("  \n"), None);
    }

    #[test]
    fn only_found_tools_count() {
        let tools = vec![
            Tool::missing("fio"),
            Tool {
                name: "tc".into(),
                found: true,
                version: None,
            },
        ];
        assert!(found(&tools, "fio").is_none());
        assert!(found(&tools, "tc").is_some());
        assert!(found(&tools, "iperf3").is_none());
    }
}
