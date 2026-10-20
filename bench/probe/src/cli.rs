//! Command line shared by `serve` and `fetch`.

use std::{path::PathBuf, str::FromStr};

use clap::{Args, Parser, Subcommand, ValueEnum};
use iroh::RelayUrl;
use iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT;
use serde::{Deserialize, Serialize};

/// Transport probe for iroh and iroh-blobs.
#[derive(Parser)]
#[command(name = "probe", version)]
pub struct Cli {
    /// What to run.
    #[command(subcommand)]
    pub command: Command,
}

/// The two roles of a measurement.
#[derive(Subcommand)]
pub enum Command {
    /// Serve generated test data, print one ticket line on stdout, and run until killed.
    Serve(ServeArgs),
    /// Run one transfer and print one JSON result line on stdout.
    Fetch(FetchArgs),
}

/// What the bytes travel over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Bytes over QUIC streams, no blobs protocol and no hashing.
    Raw,
    /// An `iroh-blobs` get, every byte verified.
    Blobs,
}

/// Congestion controller, chosen on both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Congestion {
    /// BBRv3, the Sluice default (ADR 0015).
    Bbr3,
    /// Cubic, the `noq` default.
    Cubic,
    /// NewReno.
    Newreno,
}

/// Flow control windows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    /// Whatever `noq` ships.
    Default,
    /// Sized for 10 Gbit/s at 60 ms.
    Tuned,
}

/// Where fetched blobs go.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Sink {
    /// Verify, then drop.
    Discard,
    /// Write into a file store.
    Store,
}

/// Whether the provider holds the test data in memory or on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StoreKind {
    /// In RAM, so the disk is not part of what the receiver measures. The size has to fit.
    Mem,
    /// A generated file, referenced in place by an `iroh-blobs` file store.
    Fs,
}

/// Whether the connections of one fetch share a UDP socket.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EndpointMode {
    /// One endpoint, so one socket and one socket task, carries every connection.
    Shared,
    /// One endpoint per connection.
    Separate,
}

/// Relay setting. A relay is never chosen silently (ADR 0011).
#[derive(Clone, Debug)]
pub enum Relay {
    /// Direct addresses only.
    Disabled,
    /// Exactly this relay.
    Url(RelayUrl),
}

impl FromStr for Relay {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "disabled" {
            return Ok(Self::Disabled);
        }
        RelayUrl::from_str(s)
            .map(Self::Url)
            .map_err(|e| format!("expected `disabled` or a relay URL: {e}"))
    }
}

/// Options both ends must be able to set, because the sender's congestion controller and the
/// receiver's windows both limit throughput.
#[derive(Args, Clone, Debug)]
pub struct Shared {
    /// Raw QUIC streams or an `iroh-blobs` get. `fetch` must match the ticket.
    #[arg(long, value_enum, default_value_t = Mode::Blobs)]
    pub mode: Mode,
    /// Congestion controller.
    #[arg(long, value_enum, default_value_t = Congestion::Bbr3)]
    pub congestion: Congestion,
    /// Flow control windows.
    #[arg(long, value_enum, default_value_t = WindowMode::Default)]
    pub window: WindowMode,
    /// `disabled` or a relay URL.
    #[arg(long, default_value = "disabled")]
    pub relay: Relay,
    /// A rendezvous server, `https://host[:port]`, used to learn this host's public address and for
    /// nothing else. The port is the UDP port of address discovery, 7842 if left out. Not with
    /// `--relay`.
    #[arg(long, conflicts_with = "relay")]
    pub rendezvous: Option<Rendezvous>,
    /// A PEM file with a root certificate to trust for the rendezvous server, for a self-signed one
    /// in tests. A server with a LetsEncrypt certificate never needs it.
    #[arg(long, requires = "rendezvous")]
    pub rendezvous_ca: Option<PathBuf>,
    /// UPnP, PCP and NAT-PMP. Off here, unlike in the product, so a result shows hole punching and
    /// not what the router offered.
    #[arg(long, value_enum, default_value_t = PortMapping::Off)]
    pub port_mapping: PortMapping,
}

/// Whether the endpoint may ask the router for a port mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PortMapping {
    /// No gateway probing.
    Off,
    /// UPnP, PCP and NAT-PMP.
    On,
}

/// A rendezvous server: `iroh-relay` with its relay half off (ADR 0013).
#[derive(Clone, Debug)]
pub struct Rendezvous {
    /// What the user typed, which also names the host.
    pub url: RelayUrl,
    /// UDP port of address discovery.
    pub quic_port: u16,
}

impl FromStr for Rendezvous {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = RelayUrl::from_str(s).map_err(|e| format!("not a URL: {e}"))?;
        if url.scheme() != "https" || url.host_str().is_none() {
            return Err("expected https://host[:port], address discovery is always TLS".into());
        }
        let quic_port = url.port().unwrap_or(DEFAULT_RELAY_QUIC_PORT);
        Ok(Self { url, quic_port })
    }
}

/// Options for `probe serve`.
#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Options shared with `fetch`.
    #[command(flatten)]
    pub shared: Shared,
    /// Test data size in bytes, with an optional KiB, MiB or GiB suffix. With `--store mem` it is
    /// held in memory.
    #[arg(long, default_value = "1GiB", value_parser = parse_size)]
    pub size: u64,
    /// Seed of the test data.
    #[arg(long, default_value_t = 0)]
    pub seed: u64,
    /// Endpoints to bind, each with its own socket and router. The ticket carries every address.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=16))]
    pub endpoints: u16,
    /// Where blobs mode keeps the data. `fs` needs `--dir`.
    #[arg(long, value_enum, default_value_t = StoreKind::Mem)]
    pub store: StoreKind,
    /// Directory for `--store fs`. Holds the generated data file and the blob store, and both are
    /// reused by later runs with the same seed and size.
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

/// Options for `probe fetch`.
#[derive(Args, Debug)]
pub struct FetchArgs {
    /// The line `probe serve` printed.
    pub ticket: String,
    /// Options shared with `serve`.
    #[command(flatten)]
    pub shared: Shared,
    /// Parallel connections, 1 to 16.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=16))]
    pub connections: u16,
    /// `shared` puts every connection on one endpoint. `separate` binds one endpoint per connection
    /// and dials the ticket's address `i % n` for connection `i`.
    #[arg(long, value_enum, default_value_t = EndpointMode::Shared)]
    pub endpoints: EndpointMode,
    /// What to do with verified blobs. Ignored in raw mode.
    #[arg(long, value_enum, default_value_t = Sink::Discard)]
    pub sink: Sink,
    /// Store directory for `--sink store`. Defaults to a temporary directory that is removed.
    #[arg(long)]
    pub dir: Option<PathBuf>,
    /// Seconds the transfer may take, handshakes excluded. On expiry `fetch` prints what it
    /// measured, with outcome `timeout`, and exits non-zero. A dial that has not finished in this
    /// many seconds, or that fails, prints outcome `no_path` instead.
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: u64,
    /// Print one `{"event":"progress","t":<s>,"bytes":<n>}` line on stdout each second, before
    /// the result line, so a caller can watch a transfer that has not finished. `t` counts seconds
    /// from the first request and `bytes` is the payload received so far.
    #[arg(long)]
    pub progress_lines: bool,
}

/// Parses `1073741824`, `64KiB`, `8MiB` or `1GiB`.
pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(split);
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("{s:?} does not start with a number"))?;
    let unit = unit.to_ascii_lowercase();
    let multiplier: u64 = match unit.as_str() {
        "" => 1,
        "kib" => 1 << 10,
        "mib" => 1 << 20,
        "gib" => 1 << 30,
        other => {
            return Err(format!(
                "unknown size suffix {other:?}, use KiB, MiB or GiB"
            ));
        }
    };
    match n.checked_mul(multiplier) {
        Some(0) => Err("size must be above zero".to_string()),
        Some(bytes) => Ok(bytes),
        None => Err(format!("{s:?} does not fit in 64 bits")),
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{Rendezvous, parse_size};

    #[test]
    fn a_rendezvous_url_names_the_udp_port_or_leaves_it_at_7842() {
        let plain = Rendezvous::from_str("https://rendezvous.example.net").unwrap();
        assert_eq!(plain.quic_port, 7842);
        let named = Rendezvous::from_str("https://127.0.0.1:9000").unwrap();
        assert_eq!(named.quic_port, 9000);
    }

    #[test]
    fn a_rendezvous_url_has_to_be_https_because_discovery_is_always_tls() {
        assert!(Rendezvous::from_str("http://rendezvous.example.net").is_err());
        assert!(Rendezvous::from_str("rendezvous.example.net").is_err());
    }

    #[test]
    fn sizes_parse_with_binary_suffixes() {
        assert_eq!(parse_size("1073741824"), Ok(1 << 30));
        assert_eq!(parse_size("64KiB"), Ok(64 << 10));
        assert_eq!(parse_size("8mib"), Ok(8 << 20));
        assert_eq!(parse_size("1GiB"), Ok(1 << 30));
    }

    #[test]
    fn bad_sizes_are_rejected() {
        assert!(parse_size("").is_err());
        assert!(parse_size("MiB").is_err());
        assert!(parse_size("5GB").is_err());
        assert!(parse_size("0").is_err());
        assert!(parse_size("99999999999999GiB").is_err());
    }
}
