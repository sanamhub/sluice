//! The limits `[meet]` sets and the per-IP connection cap the binary applies.

/// How many `/meet` connections one IP may hold at once. The ADR fixes this as a per-IP cap but
/// does not put it in `relay.toml`; eight leaves room for a browser tab, a retry and a few probes.
pub const DEFAULT_MAX_CONNECTIONS_PER_IP: usize = 8;

/// The limits `[meet]` sets, plus the per-IP connection cap the binary applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Claims and joins one IP may make per minute, 1..=600.
    pub rate_per_ip: u32,
    /// Plates held at once, 1..=9999.
    pub max_plates: usize,
    /// How long a plate lives, 60..=600 seconds.
    pub ttl_seconds: u64,
    /// `/meet` connections one IP may hold at once.
    pub max_connections_per_ip: usize,
}

/// A `[meet]` limit outside the range ADR 0041 sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// `rate_per_ip` is not 1..=600.
    #[error("rate_per_ip must be from 1 to 600")]
    Rate,
    /// `max_plates` is not 1..=9999.
    #[error("max_plates must be from 1 to 9999")]
    MaxPlates,
    /// `ttl_seconds` is not 60..=600.
    #[error("ttl_seconds must be from 60 to 600")]
    Ttl,
    /// The per-IP connection cap is zero.
    #[error("the per-IP connection cap must be at least one")]
    Connections,
}

impl Config {
    /// A configuration with the guard's bounds and the default per-IP connection cap.
    pub fn new(rate_per_ip: u32, max_plates: usize, ttl_seconds: u64) -> Result<Self, ConfigError> {
        if !(1..=600).contains(&rate_per_ip) {
            return Err(ConfigError::Rate);
        }
        if !(1..=9999).contains(&max_plates) {
            return Err(ConfigError::MaxPlates);
        }
        if !(60..=600).contains(&ttl_seconds) {
            return Err(ConfigError::Ttl);
        }
        Ok(Self {
            rate_per_ip,
            max_plates,
            ttl_seconds,
            max_connections_per_ip: DEFAULT_MAX_CONNECTIONS_PER_IP,
        })
    }

    /// Replaces the per-IP connection cap, for a deployment or a test with different needs.
    pub fn with_connections(mut self, max: usize) -> Result<Self, ConfigError> {
        if max == 0 {
            return Err(ConfigError::Connections);
        }
        self.max_connections_per_ip = max;
        Ok(self)
    }

    pub(super) fn ttl_ms(&self) -> u64 {
        self.ttl_seconds.saturating_mul(1000)
    }

    pub(super) fn bucket_capacity(&self) -> u64 {
        u64::from(self.rate_per_ip).saturating_mul(60_000)
    }
}
