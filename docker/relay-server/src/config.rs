//! `relay.toml`, the same schema the guard checked. Every key and every default is iroh-relay
//! 1.4.1's, plus the guard's own `[meet]` table. An unknown key is refused here too, so a config
//! the guard passed is one this binary reads the same way.

use std::net::{Ipv6Addr, SocketAddr};
use std::num::NonZeroU32;
use std::path::PathBuf;

use serde::Deserialize;
use sluice_meet::meet::Config as MeetLimits;

/// A refusal to read the config. It names a key or a rule and never a value.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The text is not TOML, or a key is not one of the schema's. The message names no value: a
    /// token may be in the file, and TOML's own errors quote what they refuse.
    #[error("the config is not the relay's schema; the guard checked a different file")]
    Schema,
    /// The relay half is off, so the image would carry nothing.
    #[error("`enable_relay` is false: the relay half must be on")]
    RelayOff,
    /// A `[tls]` key is missing.
    #[error("`[tls]` needs `cert_mode` and a certificate directory")]
    Tls,
    /// The certificate mode is one this binary does not carry.
    #[error("`cert_mode` is not Manual: ACME is not supported by sluice-relay")]
    CertMode,
    /// `[meet]` is enabled without its three limits, or one is outside ADR 0041's bounds.
    #[error("`[meet]`: {0}")]
    Meet(&'static str),
}

/// The config the listener uses, with iroh-relay's defaults applied.
#[derive(Debug)]
pub struct Config {
    pub http_bind_addr: SocketAddr,
    pub tls: Option<Tls>,
    pub metrics_addr: Option<SocketAddr>,
    pub access: AccessConfig,
    pub limits: Limits,
    pub key_cache_capacity: Option<usize>,
    pub meet: Option<MeetLimits>,
}

/// The HTTPS listener and the certificate it presents.
#[derive(Debug)]
pub struct Tls {
    pub https_bind_addr: SocketAddr,
    pub quic_bind_addr: Option<SocketAddr>,
    pub cert_dir: PathBuf,
    pub manual_cert_path: Option<PathBuf>,
    pub manual_key_path: Option<PathBuf>,
}

/// The client rate limits, as `iroh-relay` names them.
#[derive(Debug, Default)]
pub struct Limits {
    pub client_rx: Option<NonZeroU32>,
    pub max_burst_bytes: Option<NonZeroU32>,
}

/// iroh-relay 1.4.1's `access` values.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum AccessConfig {
    Everyone,
    Allowlist(Vec<String>),
    Denylist(Vec<String>),
    Http(HttpAccess),
    #[serde(rename = "shared_token")]
    SharedToken(Vec<String>),
}

/// `access.http`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpAccess {
    pub url: String,
    pub bearer_token: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    enable_relay: Option<bool>,
    http_bind_addr: Option<SocketAddr>,
    tls: Option<TlsFile>,
    #[serde(default)]
    enable_quic_addr_discovery: bool,
    limits: Option<LimitsFile>,
    enable_metrics: Option<bool>,
    metrics_bind_addr: Option<SocketAddr>,
    key_cache_capacity: Option<usize>,
    access: Option<AccessConfig>,
    meet: Option<MeetFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TlsFile {
    https_bind_addr: Option<SocketAddr>,
    quic_bind_addr: Option<SocketAddr>,
    cert_mode: CertMode,
    cert_dir: Option<PathBuf>,
    manual_cert_path: Option<PathBuf>,
    manual_key_path: Option<PathBuf>,
    #[serde(default)]
    #[allow(dead_code)]
    hostname: Hostnames,
    #[serde(default)]
    #[allow(dead_code)]
    prod_tls: bool,
    #[serde(default)]
    #[allow(dead_code)]
    contact: Option<String>,
    #[serde(default)]
    dangerous_http_only: bool,
}

#[derive(Debug, Deserialize)]
enum CertMode {
    Manual,
    LetsEncrypt,
    Reloading,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsFile {
    #[allow(dead_code)]
    accept_conn_limit: Option<f64>,
    #[allow(dead_code)]
    accept_conn_burst: Option<usize>,
    client: Option<PerClientFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PerClientFile {
    rx: Option<RateFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateFile {
    bytes_per_second: Option<u32>,
    max_burst_bytes: Option<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MeetFile {
    enable: bool,
    rate_per_ip: Option<u32>,
    max_plates: Option<u32>,
    ttl_seconds: Option<u32>,
}

/// One host name or a list, as iroh-relay reads `tls.hostname`. The binary does not use it; it is
/// here so a config with it is not refused for an unknown key.
#[derive(Debug, Default)]
struct Hostnames;

impl<'de> Deserialize<'de> for Hostnames {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        #[allow(dead_code)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        let _ = OneOrMany::deserialize(deserializer)?;
        Ok(Self)
    }
}

impl Config {
    /// Reads a config's text and applies iroh-relay's defaults.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let file: File = toml::from_str(text).map_err(|_| ConfigError::Schema)?;
        if file.enable_relay == Some(false) {
            return Err(ConfigError::RelayOff);
        }
        let http_bind_addr = file
            .http_bind_addr
            .unwrap_or_else(|| (Ipv6Addr::UNSPECIFIED, 80).into());
        let tls = match file.tls {
            Some(tls) => Some(tls.resolve(&http_bind_addr, file.enable_quic_addr_discovery)?),
            None => None,
        };
        let metrics_addr = match file.enable_metrics.unwrap_or(true) {
            true => Some(
                file.metrics_bind_addr
                    .unwrap_or_else(|| SocketAddr::new(http_bind_addr.ip(), 9090)),
            ),
            false => None,
        };
        let limits = file
            .limits
            .map_or_else(Limits::default, |limits| limits.resolve());
        let meet = file.meet.map(MeetFile::resolve).transpose()?.flatten();
        Ok(Self {
            http_bind_addr,
            tls,
            metrics_addr,
            access: file.access.unwrap_or(AccessConfig::Everyone),
            limits,
            key_cache_capacity: file.key_cache_capacity,
            meet,
        })
    }

    /// The plain-HTTP address, used only for `/generate_204` when TLS is on.
    pub fn captive_bind_addr(&self) -> SocketAddr {
        self.http_bind_addr
    }

    /// The address the relay service answers on: HTTPS when TLS is on, plain HTTP otherwise.
    pub fn relay_bind_addr(&self) -> SocketAddr {
        self.tls
            .as_ref()
            .map_or(self.http_bind_addr, |tls| tls.https_bind_addr)
    }
}

impl TlsFile {
    fn resolve(self, http: &SocketAddr, quic: bool) -> Result<Tls, ConfigError> {
        if self.dangerous_http_only {
            return Err(ConfigError::Tls);
        }
        if !matches!(self.cert_mode, CertMode::Manual) {
            return Err(ConfigError::CertMode);
        }
        let https_bind_addr = self
            .https_bind_addr
            .unwrap_or_else(|| SocketAddr::new(http.ip(), 443));
        let quic_bind_addr = quic.then(|| {
            self.quic_bind_addr
                .unwrap_or_else(|| SocketAddr::new(https_bind_addr.ip(), 7842))
        });
        let cert_dir = self.cert_dir.unwrap_or_else(|| "/etc/relay/certs".into());
        Ok(Tls {
            https_bind_addr,
            quic_bind_addr,
            cert_dir,
            manual_cert_path: self.manual_cert_path,
            manual_key_path: self.manual_key_path,
        })
    }
}

impl LimitsFile {
    fn resolve(self) -> Limits {
        let rx = self.client.and_then(|client| client.rx);
        let client_rx = rx
            .as_ref()
            .and_then(|rx| rx.bytes_per_second)
            .and_then(NonZeroU32::new);
        let max_burst_bytes = rx
            .as_ref()
            .and_then(|rx| rx.max_burst_bytes)
            .and_then(NonZeroU32::new);
        Limits {
            client_rx,
            max_burst_bytes,
        }
    }
}

impl MeetFile {
    fn resolve(self) -> Result<Option<MeetLimits>, ConfigError> {
        if !self.enable {
            return Ok(None);
        }
        let (Some(rate), Some(plates), Some(ttl)) =
            (self.rate_per_ip, self.max_plates, self.ttl_seconds)
        else {
            return Err(ConfigError::Meet(
                "enable = true needs rate_per_ip, max_plates and ttl_seconds",
            ));
        };
        MeetLimits::new(rate, plates as usize, u64::from(ttl))
            .map(Some)
            .map_err(|_| ConfigError::Meet("a limit is outside ADR 0041's bounds"))
    }
}
