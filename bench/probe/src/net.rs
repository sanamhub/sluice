//! Endpoint construction. Every measurement knob that lives below the application protocol is
//! set here, so serve and fetch cannot drift apart.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{
    Endpoint, EndpointAddr, NET_REPORT_TIMEOUT, RelayMap, RelayMode, Watcher,
    endpoint::{Connection, QuicTransportConfig, VarInt, presets},
};
use noq::congestion::{Bbr3Config, CubicConfig, NewRenoConfig};
use rustls::NamedGroup;

use crate::cli::{Congestion, Relay, Shared, WindowMode};

/// ALPN of the raw-streams protocol.
pub const RAW_ALPN: &[u8] = b"sluice-probe/raw/1";

/// 10 Gbit/s x 60 ms = 75 MB, the largest bandwidth-delay product in `docs/performance.md` (S6).
const TUNED_WINDOW_BYTES: u32 = 75_000_000;

/// `noq` 1.3.0 defaults: 12.5 MB/s x 100 ms per stream, no connection-level limit, and a send
/// window of 8 stream windows. A unit test compares these with `noq` so a change is noticed.
const DEFAULT_STREAM_RECEIVE_BYTES: u64 = 1_250_000;
const DEFAULT_RECEIVE_BYTES: u64 = (1 << 62) - 1;
const DEFAULT_SEND_BYTES: u64 = 10_000_000;

/// The window sizes in force, reported in every result.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Windows {
    /// `default` or `tuned`.
    pub mode: WindowMode,
    /// Bytes a peer may send on one stream before it must wait.
    pub stream_receive_bytes: u64,
    /// Bytes a peer may send on one connection before it must wait.
    pub receive_bytes: u64,
    /// Bytes this side keeps unacknowledged.
    pub send_bytes: u64,
}

impl Windows {
    /// The values `mode` selects.
    pub fn for_mode(mode: WindowMode) -> Self {
        match mode {
            WindowMode::Default => Self {
                mode,
                stream_receive_bytes: DEFAULT_STREAM_RECEIVE_BYTES,
                receive_bytes: DEFAULT_RECEIVE_BYTES,
                send_bytes: DEFAULT_SEND_BYTES,
            },
            WindowMode::Tuned => Self {
                mode,
                stream_receive_bytes: u64::from(TUNED_WINDOW_BYTES),
                receive_bytes: u64::from(TUNED_WINDOW_BYTES),
                send_bytes: u64::from(TUNED_WINDOW_BYTES),
            },
        }
    }
}

fn transport_config(shared: &Shared) -> QuicTransportConfig {
    let builder = QuicTransportConfig::builder();
    let builder = match shared.congestion {
        Congestion::Bbr3 => builder.congestion_controller_factory(Arc::new(Bbr3Config::default())),
        Congestion::Cubic => {
            builder.congestion_controller_factory(Arc::new(CubicConfig::default()))
        }
        Congestion::Newreno => {
            builder.congestion_controller_factory(Arc::new(NewRenoConfig::default()))
        }
    };
    let builder = match shared.window {
        WindowMode::Default => builder,
        WindowMode::Tuned => builder
            .stream_receive_window(VarInt::from_u32(TUNED_WINDOW_BYTES))
            .receive_window(VarInt::from_u32(TUNED_WINDOW_BYTES))
            .send_window(u64::from(TUNED_WINDOW_BYTES)),
    };
    builder.build()
}

/// Binds an endpoint with the `aws-lc-rs` provider passed explicitly (ADR 0012). Without address
/// lookup, and with relay mode taken from `shared`, so nothing reaches a relay unasked (ADR 0011).
pub async fn bind(shared: &Shared) -> Result<Endpoint> {
    let relay_mode = match &shared.relay {
        Relay::Disabled => RelayMode::Disabled,
        Relay::Url(url) => RelayMode::Custom(RelayMap::from(url.clone())),
    };
    Endpoint::builder(presets::Empty)
        .crypto_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .relay_mode(relay_mode)
        .transport_config(transport_config(shared))
        .bind()
        .await
        .context("bind endpoint")
}

/// The address to put in a ticket. With no relay it waits until the endpoint knows a direct
/// address, since a ticket without one cannot be dialled.
pub async fn dialable_addr(endpoint: &Endpoint, relay: &Relay) -> Result<EndpointAddr> {
    if matches!(relay, Relay::Url(_)) {
        let wait = Duration::from_secs(NET_REPORT_TIMEOUT);
        let online = tokio::time::timeout(wait, endpoint.online());
        online.await.context("the relay did not answer")?;
    }
    let mut watcher = endpoint.watch_addr();
    let mut addr = watcher.get();
    let found = async {
        while addr.ip_addrs().next().is_none() {
            addr = watcher
                .updated()
                .await
                .map_err(|_| anyhow!("endpoint closed before it had an address"))?;
        }
        Ok::<_, anyhow::Error>(addr)
    };
    let wait = Duration::from_secs(NET_REPORT_TIMEOUT);
    tokio::time::timeout(wait, found)
        .await
        .context("no direct address appeared, is any network interface up?")?
}

/// The key exchange group the handshake settled on (ADR 0012).
pub fn key_exchange_group(conn: &Connection) -> Result<NamedGroup> {
    let data = conn
        .handshake_data()
        .context("the handshake has not finished")?;
    let data = data
        .downcast_ref::<noq::crypto::rustls::HandshakeData>()
        .context("handshake data is not from rustls")?;
    data.negotiated_key_exchange_group
        .context("no key exchange group was recorded")
}

/// Fails unless the connection negotiated `X25519MLKEM768`. A `ring` provider anywhere in the
/// build would downgrade it silently, which is the failure this guards.
pub fn require_post_quantum(conn: &Connection) -> Result<NamedGroup> {
    let group = key_exchange_group(conn)?;
    if group != NamedGroup::X25519MLKEM768 {
        bail!("negotiated {group:?}, expected X25519MLKEM768");
    }
    Ok(group)
}

#[cfg(test)]
mod tests {
    use iroh::endpoint::QuicTransportConfig;

    use super::{DEFAULT_RECEIVE_BYTES, DEFAULT_SEND_BYTES, DEFAULT_STREAM_RECEIVE_BYTES};

    #[test]
    fn reported_default_windows_are_what_noq_uses() {
        let debug = format!("{:?}", QuicTransportConfig::default());
        for (field, value) in [
            ("stream_receive_window", DEFAULT_STREAM_RECEIVE_BYTES),
            ("receive_window", DEFAULT_RECEIVE_BYTES),
            ("send_window", DEFAULT_SEND_BYTES),
        ] {
            let needle = format!(" {field}: ");
            let rest = &debug[debug.find(&needle).expect(field) + needle.len()..];
            let end = rest.find([',', '}']).unwrap();
            let digits: String = rest[..end].chars().filter(char::is_ascii_digit).collect();
            assert_eq!(digits, value.to_string(), "{field} in {debug}");
        }
    }
}
