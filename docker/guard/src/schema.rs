//! iroh-relay 1.4.0's config file, as `src/main.rs` of that crate reads it, with every unknown key
//! refused at every level. iroh-relay itself ignores a key it does not know, so `[acess]` would be
//! read there as no access setting at all, which it runs as open (ADR 0019).
//!
//! Each type names the lines of `iroh-relay-1.4.0/src/main.rs` it mirrors. The field names, the
//! types, which fields may be left out and the serde shape of the enums are copied from there. Two
//! types differ, and parse the same TOML: `EndpointId` and `Url` are read as strings, so that the
//! guard checks them itself, with messages that never print them. Each field with a default that
//! opens something, `enable_relay` and `enable_metrics`, is an `Option` here, so that a missing one
//! can be named.
//!
//! The guard reads `access`, `limits`, `enable_relay`, `enable_metrics` and
//! `tls.dangerous_http_only`. The other fields are here so that a misspelling of them is refused,
//! and their defaults do not matter, only whether one may be left out, which `#[serde(default)]`
//! says as iroh-relay does.

// Most fields are never read: they exist to be parsed, and refused if misspelled.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

/// `struct Config`, main.rs:94-159.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// `true` by default there (main.rs:107, 376-378). Here a missing one stays `None`, so that
    /// rendezvous mode can require it written out (ADR 0020).
    pub enable_relay: Option<bool>,
    pub http_bind_addr: Option<SocketAddr>,
    pub tls: Option<TlsConfig>,
    #[serde(default)]
    pub enable_quic_addr_discovery: bool,
    pub limits: Option<Limits>,
    /// `true` by default there (main.rs:145, 384-386). Here a missing one stays `None`, so the
    /// guard can say that it was left out (ADR 0020).
    pub enable_metrics: Option<bool>,
    pub metrics_bind_addr: Option<SocketAddr>,
    pub key_cache_capacity: Option<usize>,
    /// `#[serde(default)]` there, with `Everyone` as the default. Here a missing `access` stays
    /// `None`, so the guard can say that it was left out.
    pub access: Option<AccessConfig>,
}

/// `enum AccessConfig`, main.rs:161-201: lower case names, and `shared_token` renamed.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum AccessConfig {
    Everyone,
    /// `Vec<EndpointId>` there.
    Allowlist(Vec<String>),
    /// `Vec<EndpointId>` there.
    Denylist(Vec<String>),
    Http(HttpAccessConfig),
    #[serde(rename = "shared_token")]
    SharedToken(Vec<String>),
}

/// `struct HttpAccessConfig`, main.rs:203-214.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HttpAccessConfig {
    /// `Url` there. The guard never reads it, and never prints it: its query may hold a secret.
    pub url: String,
    pub bearer_token: Option<String>,
}

/// `enum CertMode`, main.rs:65-71. `Reloading` is behind the `server` feature there, which the
/// binary in the image is built with.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum CertMode {
    Manual,
    LetsEncrypt,
    Reloading,
}

/// `struct TlsConfig`, main.rs:399-462. `cert_mode` has no default there, so it is required.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TlsConfig {
    pub https_bind_addr: Option<SocketAddr>,
    pub quic_bind_addr: Option<SocketAddr>,
    #[serde(default, deserialize_with = "string_or_seq")]
    pub hostname: Vec<String>,
    pub cert_mode: CertMode,
    pub cert_dir: Option<PathBuf>,
    pub manual_cert_path: Option<PathBuf>,
    pub manual_key_path: Option<PathBuf>,
    /// `true` by default there (main.rs:389-391 and 448), and `false` here, which differs. It only
    /// picks the LetsEncrypt directory, so the guard never reads it: no rule depends on what it
    /// says, and the field exists so that a misspelling of it is refused.
    #[serde(default)]
    pub prod_tls: bool,
    pub contact: Option<String>,
    #[serde(default)]
    pub dangerous_http_only: bool,
}

/// `fn string_or_seq`, main.rs:493-508: one host name, or a list of them.
fn string_or_seq<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        One(String),
        Many(Vec<String>),
    }

    Ok(match StringOrVec::deserialize(deserializer)? {
        StringOrVec::One(s) => vec![s],
        StringOrVec::Many(v) => v,
    })
}

/// `struct Limits`, main.rs:510-518.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Limits {
    pub accept_conn_limit: Option<f64>,
    pub accept_conn_burst: Option<usize>,
    pub client: Option<PerClientRateLimitConfig>,
}

/// `struct PerClientRateLimitConfig`, main.rs:527-531.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PerClientRateLimitConfig {
    pub rx: Option<RateLimitConfig>,
}

/// `struct RateLimitConfig`, main.rs:533-539.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RateLimitConfig {
    pub bytes_per_second: Option<u32>,
    pub max_burst_bytes: Option<u32>,
}
