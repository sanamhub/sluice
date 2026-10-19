//! The line `serve` prints and `fetch` reads.

use anyhow::{Context, Result, bail};
use iroh::EndpointAddr;
use serde::{Deserialize, Serialize};

use crate::cli::Mode;

const PREFIX: &str = "sluiceprobe2";

/// Everything `fetch` needs to run and to check the result without being told twice.
#[derive(Serialize, Deserialize)]
pub struct Ticket {
    /// What `serve` is serving.
    pub mode: Mode,
    /// One address per endpoint `serve` bound. Direct addresses only unless `serve` was given a
    /// relay.
    pub addrs: Vec<EndpointAddr>,
    /// Size of the test data in bytes.
    pub size: u64,
    /// Seed of the test data, so `fetch` can compute the expected root itself.
    pub seed: u64,
    /// BLAKE3 root as `iroh-blobs` computed it on import. Blobs mode only.
    pub hash: Option<String>,
}

impl Ticket {
    /// Hex of the JSON, so it survives a shell without quoting.
    pub fn encode(&self) -> Result<String> {
        let json = serde_json::to_vec(self).context("serialize ticket")?;
        let hex: String = json.iter().map(|b| format!("{b:02x}")).collect();
        Ok(format!("{PREFIX}{hex}"))
    }

    /// Inverse of [`Ticket::encode`].
    pub fn decode(line: &str) -> Result<Self> {
        let Some(hex) = line.trim().strip_prefix(PREFIX) else {
            bail!("not a probe ticket: it must start with {PREFIX}");
        };
        if !hex.len().is_multiple_of(2) || !hex.is_ascii() {
            bail!("ticket is not valid hex");
        }
        let json = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .context("ticket is not valid hex")?;
        serde_json::from_slice(&json).context("ticket contents are malformed")
    }
}

#[cfg(test)]
mod tests {
    use iroh::{EndpointId, SecretKey};

    use super::Ticket;
    use crate::cli::Mode;

    #[test]
    fn roundtrip() {
        let id: EndpointId = SecretKey::generate().public();
        let ticket = Ticket {
            mode: Mode::Blobs,
            addrs: vec![id.into()],
            size: 8 << 20,
            seed: 3,
            hash: Some("ab".repeat(32)),
        };
        let decoded = Ticket::decode(&ticket.encode().unwrap()).unwrap();
        assert_eq!(decoded.mode, Mode::Blobs);
        assert_eq!(decoded.addrs.len(), 1);
        assert_eq!(decoded.addrs[0].id, id);
        assert_eq!((decoded.size, decoded.seed), (8 << 20, 3));
        assert_eq!(decoded.hash, ticket.hash);
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Ticket::decode("nope").is_err());
        assert!(Ticket::decode("sluiceprobe2zz").is_err());
    }
}
