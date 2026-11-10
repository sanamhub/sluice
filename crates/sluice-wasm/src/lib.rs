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

use iroh::{Endpoint, EndpointId, RelayMap, RelayMode, RelayUrl, endpoint::presets};
use iroh_blobs::{Hash, HashAndFormat, api::downloader::Shuffled, store::mem::MemStore};
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

/// Fetches the blob with `root` from `sender` through `relay_url`, verifies it and returns its
/// bytes.
///
/// The bytes are checked against `root` on the way in: every block is a BLAKE3 proof, and a block
/// that does not match is refused before it reaches the caller. A blob that arrives is the blob the
/// root names, so the page can write it without checking it again.
///
/// It compiles for the target and runs, and it has not yet downloaded anything: against a relay on
/// this machine it fails with `Unable to download <root>`, and whether that is the relay's WebSocket
/// path in Node or the way the peer is looked up is not settled. `docs/browser.md` records it.
///
/// # Errors
///
/// A relay URL, a sender id or a root that is not one, an endpoint that cannot be bound, and a
/// fetch that fails are all strings for the caller to show. None of them quotes the ticket.
#[wasm_bindgen]
pub async fn fetch_blob(relay_url: &str, sender: &str, root: &str) -> Result<Vec<u8>, JsValue> {
    let url = relay(relay_url).map_err(to_js)?;
    let sender = sender_id(sender).map_err(to_js)?;
    let hash = hash(root).map_err(to_js)?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_only(url))
        .bind()
        .await
        .map_err(|e| to_js(Bad(format!("cannot open an endpoint: {e}"))))?;
    // The relay connection has to be up before a peer can be found through it: a browser has no
    // direct path, so the relay is the only way to the sender.
    endpoint.online().await;
    let store = MemStore::new();
    // The downloader takes the peers that may have the content; the endpoint id is enough, because
    // a relayed connection is the only kind a browser makes and the relay knows where it lives.
    store
        .downloader(&endpoint)
        .download(HashAndFormat::raw(hash), Shuffled::new(vec![sender]))
        .await
        .map_err(|e| to_js(Bad(format!("the fetch stopped: {e}"))))?;
    let bytes = store
        .blobs()
        .get_bytes(hash)
        .await
        .map_err(|e| to_js(Bad(format!("the blob cannot be read: {e}"))))?;
    endpoint.close().await;
    Ok(bytes.to_vec())
}

/// The endpoint id a ticket names, from its 64 hex characters.
fn sender_id(text: &str) -> Result<EndpointId, Bad> {
    let bad = || Bad("a sender is 64 hex characters".to_string());
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let mut bytes = [0u8; 32];
    for (index, pair) in text.as_bytes().chunks(2).enumerate() {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        bytes[index] = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    EndpointId::from_bytes(&bytes).map_err(|_| bad())
}

/// A refusal as the value a page catches.
fn to_js(error: Bad) -> JsValue {
    JsValue::from_str(&error.0)
}
