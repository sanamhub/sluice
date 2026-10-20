//! The rendezvous server a `bench connect` or `bench check` run points the probe at.

use std::{
    net::{IpAddr, ToSocketAddrs},
    time::Duration,
};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::{
    check::LABEL,
    host::{Host, quote},
};

/// UDP port of address discovery when the URL names none, as in `iroh-relay`.
const DEFAULT_PORT: u16 = 7842;
const TICKET_PREFIX: &str = "sluiceprobe2";
/// `serve` waits up to 10 s for the server, then prints its ticket anyway.
const TICKET_WAIT: Duration = Duration::from_secs(60);
const QUICK: Duration = Duration::from_secs(30);

/// Whether the probe may ask the router for a port mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortMapping {
    /// No gateway probing.
    Off,
    /// UPnP, PCP and NAT-PMP.
    On,
}

impl PortMapping {
    /// The word the probe's `--port-mapping` takes.
    pub fn word(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
        }
    }
}

/// A rendezvous server: `https://host[:port]`, where the port is the UDP port of address
/// discovery.
#[derive(Clone, Debug)]
pub struct Rendezvous {
    url: String,
    ca: Option<String>,
    host: String,
    port: u16,
}

impl Rendezvous {
    /// `ca` is a path the probe opens on each host, so it has to exist at that path on both.
    pub fn parse(url: &str, ca: Option<String>) -> Result<Self> {
        let Some(rest) = url.strip_prefix("https://") else {
            bail!(
                "the rendezvous URL has to be https://host[:port], address discovery is always TLS"
            );
        };
        let authority = rest.split('/').next().unwrap_or_default();
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (addr, tail) = v6
                    .split_once(']')
                    .ok_or_else(|| anyhow::anyhow!("unclosed [ in {url}"))?;
                (addr, tail.strip_prefix(':'))
            }
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            },
        };
        if host.is_empty() {
            bail!("no host in {url}");
        }
        let port = match port {
            Some(p) => p
                .parse()
                .map_err(|_| anyhow::anyhow!("{p:?} is not a port in {url}"))?,
            None => DEFAULT_PORT,
        };
        Ok(Self {
            url: url.to_string(),
            ca,
            host: host.to_string(),
            port,
        })
    }

    /// The URL as given.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The probe options that point it at this server.
    pub fn args(&self, port_mapping: PortMapping) -> Vec<String> {
        let mut args = vec!["--rendezvous".to_string(), self.url.clone()];
        if let Some(ca) = &self.ca {
            args.extend(["--rendezvous-ca".to_string(), ca.clone()]);
        }
        args.extend([
            "--port-mapping".to_string(),
            port_mapping.word().to_string(),
        ]);
        args
    }
}

/// The `discovery` object inside a probe ticket line, which is the hex of a JSON document.
pub fn ticket_discovery(ticket: &str) -> Option<Value> {
    let hex = ticket.trim().strip_prefix(TICKET_PREFIX)?;
    if !hex.is_ascii() || hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect();
    let document: Value = serde_json::from_slice(&bytes?).ok()?;
    let discovery = document.get("discovery")?.clone();
    (!discovery.is_null()).then_some(discovery)
}

/// The addresses a `discovery` object lists.
pub fn learned(discovery: Option<&Value>) -> Vec<String> {
    discovery
        .and_then(|d| d["learned"].as_array())
        .map(|list| {
            list.iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Prints what `host` can do with the server and returns false if it cannot reach it: whether the
/// name resolves, and whether a probe there gets an address back over UDP.
pub async fn check(host: &Host, rendezvous: &Rendezvous) -> bool {
    let label = "rendezvous";
    let resolved = resolve(host, rendezvous).await;
    match &resolved {
        Ok(address) => println!("  {label:<LABEL$} resolves: ok, {address}"),
        Err(reason) => println!("  {label:<LABEL$} resolves: FAIL, {reason}"),
    }
    if resolved.is_err() {
        return false;
    }
    let port = rendezvous.port;
    match ask(host, rendezvous).await {
        Ok(line) => {
            println!("  {label:<LABEL$} udp {port}: ok, {line}");
            true
        }
        Err(reason) => {
            println!("  {label:<LABEL$} udp {port}: FAIL, {reason}");
            false
        }
    }
}

async fn resolve(host: &Host, rendezvous: &Rendezvous) -> Result<String> {
    let name = rendezvous.host.clone();
    if name.parse::<IpAddr>().is_ok() {
        return Ok("an address, no lookup needed".to_string());
    }
    if host.is_local() {
        let port = rendezvous.port;
        let found = tokio::task::spawn_blocking(move || {
            (name.as_str(), port)
                .to_socket_addrs()
                .map(|mut addrs| addrs.next())
        })
        .await?;
        return match found {
            Ok(Some(addr)) => Ok(addr.ip().to_string()),
            Ok(None) => bail!("no address"),
            Err(e) => bail!("{e}"),
        };
    }
    let name = quote(&name);
    // getent on Linux, dscacheutil on macOS. Both read the system resolver, as the probe does.
    let script = format!("getent ahosts {name} || dscacheutil -q host -a name {name}");
    let out = host.run(&["sh", "-c", &script], QUICK).await?;
    out.stdout
        .split_whitespace()
        .find(|word| word.parse::<IpAddr>().is_ok())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the host's resolver has no address for it"))
}

/// Starts a `serve` against the server and reads what it learnt, which is a round trip over UDP
/// and a TLS handshake.
async fn ask(host: &Host, rendezvous: &Rendezvous) -> Result<String> {
    let mut argv = vec![
        host.entry.probe.clone(),
        "serve".to_string(),
        "--mode".to_string(),
        "raw".to_string(),
        "--size".to_string(),
        "1MiB".to_string(),
    ];
    argv.extend(rendezvous.args(PortMapping::Off));
    let mut serve = host.spawn(&argv)?;
    let line = serve.next_line(TICKET_WAIT).await;
    let stderr = serve.stderr_tail();
    serve.kill().await;
    let ticket = match line {
        Ok(Some(ticket)) => ticket,
        Ok(None) => bail!("the probe exited: {stderr}"),
        Err(e) => bail!("{e}"),
    };
    let discovery = ticket_discovery(&ticket);
    let addresses = learned(discovery.as_ref());
    if discovery.as_ref().is_some_and(|d| d["complete"] == true) && !addresses.is_empty() {
        let took = discovery.as_ref().and_then(|d| d["took_s"].as_f64());
        let took = took.map_or(String::new(), |s| format!(" in {s:.1} s"));
        return Ok(format!(
            "the server reported {}{took}",
            addresses.join(", ")
        ));
    }
    bail!(
        "no answer within 10 s. The UDP port may be blocked, the server down, or its certificate \
         not trusted (--rendezvous-ca). {}",
        stderr.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_udp_port_defaults_to_the_one_iroh_relay_uses() {
        let rv = Rendezvous::parse("https://rendezvous.example.net", None).unwrap();
        assert_eq!(
            (rv.host.as_str(), rv.port),
            ("rendezvous.example.net", 7842)
        );
        let rv = Rendezvous::parse("https://rendezvous.example.net:9000/", None).unwrap();
        assert_eq!(rv.port, 9000);
        let rv = Rendezvous::parse("https://[2001:db8::1]:9000", None).unwrap();
        assert_eq!((rv.host.as_str(), rv.port), ("2001:db8::1", 9000));
        let rv = Rendezvous::parse("https://[2001:db8::1]", None).unwrap();
        assert_eq!(rv.port, 7842);
    }

    #[test]
    fn only_https_urls_with_a_host_and_a_port_that_is_a_number_are_taken() {
        assert!(Rendezvous::parse("http://rendezvous.example.net", None).is_err());
        assert!(Rendezvous::parse("rendezvous.example.net", None).is_err());
        assert!(Rendezvous::parse("https://", None).is_err());
        assert!(Rendezvous::parse("https://host:port", None).is_err());
        assert!(Rendezvous::parse("https://[::1", None).is_err());
    }

    #[test]
    fn the_probe_options_name_the_server_the_ca_and_the_port_mapping() {
        let ca = Some("/etc/rv.pem".to_string());
        let rv = Rendezvous::parse("https://rendezvous.example.net:7842", ca).unwrap();
        assert_eq!(
            rv.args(PortMapping::On).join(" "),
            "--rendezvous https://rendezvous.example.net:7842 --rendezvous-ca /etc/rv.pem --port-mapping on"
        );
        let rv = Rendezvous::parse("https://rendezvous.example.net", None).unwrap();
        assert_eq!(
            rv.args(PortMapping::Off).join(" "),
            "--rendezvous https://rendezvous.example.net --port-mapping off"
        );
    }

    #[test]
    fn a_ticket_line_gives_up_its_discovery_object() {
        let json = r#"{"addrs":[],"discovery":{"took_s":0.4,"complete":true,"learned":["203.0.113.9:4242"]}}"#;
        let hex: String = json.bytes().map(|b| format!("{b:02x}")).collect();
        let ticket = format!("sluiceprobe2{hex}");
        let discovery = ticket_discovery(&ticket);
        assert_eq!(learned(discovery.as_ref()), vec!["203.0.113.9:4242"]);
        let without: String = r#"{"discovery":null}"#.bytes().map(|b| format!("{b:02x}")).collect();
        assert!(ticket_discovery(&format!("sluiceprobe2{without}")).is_none());
        assert!(ticket_discovery("not a ticket").is_none());
        assert!(ticket_discovery("sluiceprobe2zz").is_none());
    }
}
