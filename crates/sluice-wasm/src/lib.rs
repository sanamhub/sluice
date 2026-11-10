//! The browser half of Sluice.
//!
//! A tab reaches a peer through a relay and never directly (ADR 0011), so the two things it needs
//! before anything else are a relay URL parsed into the form `iroh` dials with, and an endpoint that
//! uses it. Both are here, and they are the same on the guest and on the host, which is what lets
//! this crate be checked by the gates and built for `wasm32-unknown-unknown`.
//!
//! This crate is the first step of the browser build and not the browser build: there is no
//! wasm-bindgen export, no page and no fetch of a blob yet. What it establishes is the dependency
//! set. The native binary asks `iroh` for `tls-aws-lc-rs`, which cannot build for the target; a
//! tab reaches its relay over the WebSocket the browser already has, so the wasm build needs no TLS
//! provider, and with `default-features = false` and no TLS feature the pinned `iroh` and
//! `iroh-blobs` build for `wasm32-unknown-unknown` here. `docs/browser.md` records the measurements.

use iroh::{Endpoint, RelayMap, RelayMode, RelayUrl, endpoint::presets};
use iroh_blobs::{Hash, store::mem::MemStore};
use wasm_bindgen::prelude::*;

/// The error of a relay URL or a hash a ticket names that this build cannot read.
///
/// It is a string and not one of `sluice_core`'s variants: this crate does not depend on the core,
/// which brings the filesystem and the native crypto with it, and a browser has neither.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bad(pub String);

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Bad {}

/// The relay a URL names, in the form `iroh` dials with. The URL never appears in the error: a
/// relay URL can name a private host, and the message is for a page to show.
pub fn relay(url: &str) -> Result<RelayUrl, Bad> {
    url.parse()
        .map_err(|_| Bad("a relay URL is an absolute https or http URL".to_string()))
}

/// A relay mode for `url`, which is the only transport a browser build uses.
pub fn relay_only(url: RelayUrl) -> RelayMode {
    RelayMode::Custom(RelayMap::from(url))
}

/// The store a tab fetches into. It is memory, and a page that wants a file larger than its tab can
/// hold streams it to disk through the File System Access API or OPFS instead, which is the next
/// step and not this one.
pub fn store() -> MemStore {
    MemStore::new()
}

/// The BLAKE3 root a ticket or a manifest names, parsed from its 64 hex characters.
///
/// The length and the characters are checked here, before the parser sees the text: the parser
/// behind `Hash` asserts on a length other than 64 rather than answering with an error, and a page
/// that hands a tab a pasted root must not be able to stop it with a short string.
pub fn hash(text: &str) -> Result<Hash, Bad> {
    let bad = || Bad("a root is 64 hex characters".to_string());
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad());
    }
    text.parse().map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use super::{Bad, hash, relay, relay_only};

    /// A relay is read the one way `iroh` reads one, and a bad one is refused with a sentence and
    /// not with its text.
    #[test]
    fn a_relay_url_is_read_or_refused_without_quoting_it() {
        assert!(relay("https://relay.example/").is_ok());
        let error = relay("relay example").unwrap_err();
        assert_eq!(
            error,
            Bad("a relay URL is an absolute https or http URL".to_string())
        );
        assert!(!error.to_string().contains("relay example"));
        let url = relay("http://127.0.0.1:3340/").unwrap();
        assert!(matches!(relay_only(url), iroh::RelayMode::Custom(_)));
    }

    /// A root is 64 hex characters, and a short one is refused.
    #[test]
    fn a_root_is_sixty_four_hex_characters() {
        let zeros = "0".repeat(64);
        assert_eq!(hash(&zeros).unwrap().as_bytes(), &[0; 32]);
        assert!(hash("0").is_err());
        assert!(hash(&"g".repeat(64)).is_err());
    }
}

/// Opens an endpoint that reaches the network through `relay` and returns the id a ticket would
/// name, as 64 lower-case hex characters.
///
/// This is the first thing a tab has to be able to do, and it is exported so the packaging can be
/// tried in a page or in Node before anything is built on it. The endpoint is closed before the
/// promise resolves: this is a probe of the transport, not a session.
///
/// # Errors
///
/// A relay URL that is not one, or an endpoint that cannot be bound, is a string for the caller to
/// show. The URL never appears in it: a relay can name a private host.
#[wasm_bindgen]
pub async fn endpoint_id(relay_url: &str) -> Result<String, JsValue> {
    let url = relay(relay_url).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_only(url))
        .bind()
        .await
        .map_err(|e| JsValue::from_str(&format!("cannot open an endpoint: {e}")))?;
    let id = endpoint.id().to_string();
    endpoint.close().await;
    Ok(id)
}

/// Whether a string is a relay URL this build can dial, for a page that checks a field before it
/// starts anything.
#[wasm_bindgen]
pub fn relay_reads(text: &str) -> bool {
    relay(text).is_ok()
}
