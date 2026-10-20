//! `bench check`: what each host has and lacks. It changes nothing.

use std::{path::Path, time::Duration};

use anyhow::Result;

use crate::{
    host::{Ctx, Host, Os, quote},
    hosts::Hosts,
    tools::{self, Tool, found},
};

const QUICK: Duration = Duration::from_secs(30);

// Wide enough for `sudo -n tc`, the longest label. At 10 it filled the column and ran into its result.
const LABEL: usize = 12;

/// What each optional tool is for, so a missing one is not a mystery.
fn purpose(name: &str) -> &'static str {
    match name {
        "iperf3" => "network ceiling, needed on both hosts",
        "fio" => "disk ceilings, needed on the host whose disk a run touches",
        "tc" => "netem, needed on a Linux receiver",
        "mscp" => "parallel TCP baseline, needed on both hosts",
        _ => "",
    }
}

/// Prints a report for both hosts and returns true if a run can start: both hosts reachable,
/// the probe present, and the data directories writable.
pub async fn run(hosts: &Hosts) -> Result<bool> {
    let ctx = Ctx::default();
    let mut ready = true;
    for (role, entry) in [("sender", &hosts.sender), ("receiver", &hosts.receiver)] {
        println!("{role}: {}", entry.host);
        match Host::connect(role, entry, &ctx).await {
            Ok(host) => ready &= report(&host).await?,
            Err(e) => {
                println!("  unreachable: {e:#}");
                ready = false;
            }
        }
    }
    if hosts.sender.host == hosts.receiver.host {
        println!(
            "\nnote: both ends are {}. netem scenarios are skipped, because one interface cannot be \
             shaped in both directions.",
            hosts.sender.host
        );
    }
    println!(
        "\n{}",
        if ready {
            "A run can start. Anything marked missing is recorded in the results as not measured."
        } else {
            "A run cannot start until the lines marked FAIL are fixed."
        }
    );
    Ok(ready)
}

async fn report(host: &Host) -> Result<bool> {
    println!("  os: {:?}", host.os);
    let tools = tools::detect(host).await?;
    let mut ready = true;
    for tool in &tools {
        ready &= line(host, tool);
    }
    let writable = data_dir_writable(host).await;
    println!(
        "  {:<LABEL$} {}",
        "data_dir",
        if writable {
            format!("ok, {} is writable", host.entry.data_dir)
        } else {
            format!("FAIL, {} is not a writable directory", host.entry.data_dir)
        }
    );
    ready &= writable;
    if let Some(interface) = &host.entry.interface {
        let exists = interface_exists(host, interface).await;
        println!(
            "  {:<LABEL$} {}",
            "interface",
            if exists {
                format!("ok, {interface}")
            } else {
                format!("not found: {interface}")
            }
        );
    } else {
        println!(
            "  {:<LABEL$} not set, netem scenarios need it on both hosts",
            "interface"
        );
    }
    if found(&tools, "tc").is_some() && host.os == Os::Linux {
        let out = host.run(&["sudo", "-n", "tc", "-V"], QUICK).await;
        let allowed = out.is_ok_and(|o| o.success());
        println!(
            "  {:<LABEL$} {}",
            "sudo -n tc",
            if allowed {
                "ok"
            } else {
                "not allowed without a password, so netem scenarios cannot run"
            }
        );
    }
    if host.os != Os::Linux {
        println!(
            "  note: netem scenarios are skipped, because the {} is {:?}",
            host.role, host.os
        );
    }
    Ok(ready)
}

/// Prints one tool and returns false only for a missing probe, the one tool a run cannot do
/// without.
fn line(host: &Host, tool: &Tool) -> bool {
    let name = tool.name.as_str();
    if tool.found {
        let version = tool.version.as_deref().unwrap_or("");
        let at = if name == "probe" {
            format!(" ({})", host.entry.probe)
        } else {
            String::new()
        };
        println!("  {name:<LABEL$} ok, {version}{at}");
        true
    } else if name == "probe" {
        println!(
            "  {name:<LABEL$} FAIL, not runnable at {}",
            host.entry.probe
        );
        false
    } else {
        println!("  {name:<LABEL$} missing, used for the {}", purpose(name));
        true
    }
}

async fn data_dir_writable(host: &Host) -> bool {
    let dir = &host.entry.data_dir;
    if host.os == Os::Windows {
        return Path::new(dir).is_dir();
    }
    let script = format!("test -d {0} && test -w {0}", quote(dir));
    host.run(&["sh", "-c", &script], QUICK)
        .await
        .is_ok_and(|o| o.success())
}

async fn interface_exists(host: &Host, interface: &str) -> bool {
    let script = match host.os {
        Os::Linux => format!("test -d /sys/class/net/{}", quote(interface)),
        Os::Macos => format!("ifconfig {} >/dev/null 2>&1", quote(interface)),
        _ => return false,
    };
    host.run(&["sh", "-c", &script], QUICK)
        .await
        .is_ok_and(|o| o.success())
}
