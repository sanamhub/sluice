//! The network checks of `sluice doctor --network`. They report what this machine's network looks
//! like and never change a setting: a relay, a NAT, or a router that will not open a port is a
//! fact to say, not a reason to fail the command.

use std::net::UdpSocket;
use std::time::Duration;

use sluice_core::doctor::Status;
use sluice_core::engine::{NetworkReport, RelayModeChoice, network_report};

use crate::doctor::Line;

/// How long the relay gets to answer. The net report needs one round trip, and a blocked network
/// gives up long before this.
const RELAY_WAIT: Duration = Duration::from_secs(20);
/// How long a router gets to answer a port-mapping probe.
const PORTMAP_WAIT: Duration = Duration::from_secs(5);

/// Every network line, in a fixed order, after the local checks.
pub async fn lines(relay: &RelayModeChoice) -> Vec<Line> {
    let mut lines = vec![udp()];
    match relay {
        RelayModeChoice::Disabled => {
            lines.push(Line {
                name: "relay",
                status: Status::Skipped,
                detail: "none; across the internet only a sender whose router opens a port \
                         (UPnP, PCP or NAT-PMP) can be reached, and two sides on one network can \
                         still find each other directly"
                    .to_string(),
                fix: Some(
                    "pass --relay URL to check one, or run a relay as a meeting point, so two \
                     sides can find a direct path"
                        .to_string(),
                ),
            });
            lines.push(skipped(
                "address",
                "no relay was configured, so no public address was learnt",
            ));
            lines.push(skipped(
                "nat",
                "no relay was configured, so the NAT mapping type is unknown",
            ));
        }
        choice => match tokio::time::timeout(RELAY_WAIT, network_report(choice)).await {
            Ok(Ok(report)) => lines.extend(from_report(&report)),
            Ok(Err(error)) => lines.push(Line {
                name: "relay",
                status: Status::Warning,
                detail: error.to_string(),
                fix: Some("check the relay URL and this network's access to it".to_string()),
            }),
            Err(_) => lines.push(Line {
                name: "relay",
                status: Status::Warning,
                detail: format!("no answer within {} s", RELAY_WAIT.as_secs()),
                fix: Some("check the relay URL and this network's access to it".to_string()),
            }),
        },
    }
    lines.push(portmap().await);
    lines
}

fn udp() -> Line {
    match UdpSocket::bind(("0.0.0.0", 0)) {
        Ok(socket) => Line {
            name: "udp",
            status: Status::Ok,
            detail: match socket.local_addr() {
                Ok(addr) => format!("a UDP socket can be bound on port {}", addr.port()),
                Err(_) => "a UDP socket can be bound".to_string(),
            },
            fix: None,
        },
        Err(error) => Line {
            name: "udp",
            status: Status::Warning,
            detail: format!("no UDP socket can be bound: {error}"),
            fix: Some("a firewall or another program is holding every local UDP port".to_string()),
        },
    }
}

fn from_report(report: &NetworkReport) -> Vec<Line> {
    vec![
        udp_line(report),
        address_line(report),
        nat_line(report),
        relay_line(report),
    ]
}

fn udp_line(report: &NetworkReport) -> Line {
    Line {
        name: "udp",
        status: if report.has_udp() {
            Status::Ok
        } else {
            Status::Warning
        },
        detail: format!(
            "UDP with a relay: IPv4 {} , IPv6 {}",
            yes(report.udp_v4),
            yes(report.udp_v6)
        ),
        fix: if report.has_udp() {
            None
        } else {
            Some("a firewall may be blocking outgoing UDP".to_string())
        },
    }
}

fn address_line(report: &NetworkReport) -> Line {
    Line {
        name: "address",
        status: if report.global_v4.is_some() || report.global_v6.is_some() {
            Status::Ok
        } else {
            Status::Warning
        },
        detail: match (report.global_v4, report.global_v6) {
            (Some(v4), Some(v6)) => format!("{v4} and {v6}"),
            (Some(v4), None) => v4.to_string(),
            (None, Some(v6)) => v6.to_string(),
            (None, None) => "the relay saw no public address".to_string(),
        },
        fix: None,
    }
}

fn nat_line(report: &NetworkReport) -> Line {
    let (status, detail) = match report.mapping_varies {
        Some(true) => (
            Status::Warning,
            "each relay saw a different public address (a hard NAT), so a direct path may fail",
        ),
        Some(false) => (
            Status::Ok,
            "the same public address to every relay (a cone NAT), so a direct path can work",
        ),
        None => (
            Status::Skipped,
            "only one relay answered, so the mapping type is unknown",
        ),
    };
    Line {
        name: "nat",
        status,
        detail: detail.to_string(),
        fix: None,
    }
}

fn relay_line(report: &NetworkReport) -> Line {
    Line {
        name: "relay",
        status: Status::Ok,
        detail: match &report.preferred_relay {
            Some(url) => format!("reachable; the closest is {url}"),
            None => "reachable".to_string(),
        },
        fix: None,
    }
}

async fn portmap() -> Line {
    let client = portmapper::Client::default();
    match tokio::time::timeout(PORTMAP_WAIT, client.probe()).await {
        Ok(Ok(Ok(found))) if found.upnp || found.pcp || found.nat_pmp => Line {
            name: "portmap",
            status: Status::Ok,
            detail: format!(
                "a router can open a port (UPnP {}, PCP {}, NAT-PMP {})",
                yes(found.upnp),
                yes(found.pcp),
                yes(found.nat_pmp)
            ),
            fix: None,
        },
        Ok(Ok(Ok(_))) => Line {
            name: "portmap",
            status: Status::Warning,
            detail: "no router answered a mapping request, so no port can be opened".to_string(),
            fix: Some(
                "a router with UPnP, PCP or NAT-PMP can open one; without one, two sides on one \
                 network still find each other directly, and across the internet a relay as a \
                 meeting point is what lets the two sides meet"
                    .to_string(),
            ),
        },
        Ok(Ok(Err(error))) => Line {
            name: "portmap",
            status: Status::Warning,
            detail: format!("the mapping probe failed: {error}"),
            fix: None,
        },
        Ok(Err(error)) => Line {
            name: "portmap",
            status: Status::Warning,
            detail: format!("the mapping service stopped: {error}"),
            fix: None,
        },
        Err(_) => Line {
            name: "portmap",
            status: Status::Warning,
            detail: format!(
                "the mapping probe timed out after {} s",
                PORTMAP_WAIT.as_secs()
            ),
            fix: None,
        },
    }
}

fn skipped(name: &'static str, detail: &str) -> Line {
    Line {
        name,
        status: Status::Skipped,
        detail: detail.to_string(),
        fix: None,
    }
}

fn yes(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
