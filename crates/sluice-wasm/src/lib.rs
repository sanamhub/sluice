//! The browser half of Sluice.
//!
//! A tab reaches a peer through a relay and never directly (ADR 0011), so the two things it needs
//! before anything else are a relay URL parsed into the form `iroh` dials with, and an endpoint that
//! uses it. Both are here, and they are the same on the guest and on the host, which is what lets
//! this crate be checked by the gates and built for `wasm32-unknown-unknown`.
//!
//! This crate is the first step of the browser build and not the browser build: it exports a probe
//! of the transport and one blob fetch, and there is no app, no page of its own and no write to a
//! tab's storage. The native binary asks `iroh` for `tls-aws-lc-rs`, which has no build for the
//! target, so the wasm build takes `ring` and compiles its C for the target. `ring` has no ML-KEM
//! group, so a tab negotiates classical `X25519`, and a native peer that applies the post-quantum
//! rule refuses it; the browser side stays a proof until the browser has a post-quantum provider.
//! `docs/browser.md` records the measurements and the rule.

use std::time::Duration;

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl,
    address_lookup::MemoryLookup, endpoint::presets,
};
use iroh_blobs::{
    Hash, HashAndFormat, api::downloader::Shuffled, api::proto::BlobStatus, store::mem::MemStore,
};
use n0_future::time::timeout;
use wasm_bindgen::prelude::*;

/// How long a tab waits for its relay before it gives up. A relay that has not answered in this
/// time is not going to carry the transfer, and a page should show a failure rather than wait.
const RELAY_WAIT: Duration = Duration::from_secs(20);

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
    use super::{Bad, byte_count, hash, relay, relay_only};

    /// A ticket's size is a whole number of bytes a page can pass as a number, and nothing else.
    #[test]
    fn a_size_is_a_whole_number_of_bytes() {
        assert_eq!(byte_count(0.0).unwrap(), 0);
        assert_eq!(byte_count(2_097_152.0).unwrap(), 2_097_152);
        for bad in [-1.0, 1.5, f64::NAN, f64::INFINITY, 1e30] {
            assert!(byte_count(bad).is_err(), "{bad}");
        }
    }

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
/// bytes, when the content is the `size` the ticket names.
///
/// The bytes are checked against `root` on the way in: every block is a BLAKE3 proof, and a block
/// that does not match is refused before it reaches the caller. A blob that arrives is the blob the
/// root names, so the page can write it without checking it again.
///
/// `size` is the length the ticket promises, as a number. A blob of another length is refused
/// before its bytes are copied out of the store, so a ticket cannot make a page hold content of a
/// length it was not told about. The relay has [`RELAY_WAIT`] to answer, and the endpoint is closed
/// on every way out, including a refusal.
///
/// It has fetched from a native `sluice send` through a relay, in a headless browser: a 2 MiB file
/// whose SHA-256 matched the sender's. The address it needs is the ticket's relay, which names where
/// the sender is, because a tab can reach nothing directly. `docs/browser.md` has the run.
///
/// # Errors
///
/// A relay URL, a sender id, a root or a size that is not one, a relay that does not answer in time,
/// an endpoint that cannot be bound, a fetch that fails, and content of another length are all
/// strings for the caller to show. None of them quotes the ticket.
#[wasm_bindgen]
pub async fn fetch_blob(
    relay_url: &str,
    sender: &str,
    root: &str,
    size: f64,
) -> Result<Vec<u8>, JsValue> {
    let url = relay(relay_url).map_err(to_js)?;
    let sender = sender_id(sender).map_err(to_js)?;
    let hash = hash(root).map_err(to_js)?;
    let size = byte_count(size).map_err(to_js)?;
    // The only address a tab can use is the relay, and the ticket already says which one the sender
    // is on. Without this, a browser has an id and no way to dial it: there is no discovery service
    // configured and a tab can reach nothing directly.
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(EndpointAddr::new(sender).with_relay_url(url.clone()));
    let endpoint = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_only(url))
        .address_lookup(lookup)
        .bind()
        .await
        .map_err(|e| to_js(Bad(format!("cannot open an endpoint: {e}"))))?;
    let outcome = fetch_one(&endpoint, sender, hash, size).await;
    // Closed on every way out: a page that keeps an endpoint open holds a socket and a relay
    // registration for as long as it lives.
    endpoint.close().await;
    outcome
}

/// One fetch through an endpoint that is already bound. The store is this call's and is dropped with
/// it, so the bytes exist in the page only as the value returned here.
async fn fetch_one(
    endpoint: &Endpoint,
    sender: EndpointId,
    hash: Hash,
    size: u64,
) -> Result<Vec<u8>, JsValue> {
    // The relay connection has to be up before a peer can be found through it: a browser has no
    // direct path, so the relay is the only way to the sender.
    timeout(RELAY_WAIT, endpoint.online()).await.map_err(|_| {
        to_js(Bad(format!(
            "the relay did not answer in {} s",
            RELAY_WAIT.as_secs()
        )))
    })?;
    let store = MemStore::new();
    // The downloader takes the peers that may have the content; the endpoint id is enough, because
    // a relayed connection is the only kind a browser makes and the relay knows where it lives.
    store
        .downloader(endpoint)
        .download(HashAndFormat::raw(hash), Shuffled::new(vec![sender]))
        .await
        .map_err(|e| to_js(Bad(format!("the fetch stopped: {e}"))))?;
    let stored = match store.blobs().status(hash).await {
        Ok(BlobStatus::Complete { size }) => size,
        Ok(other) => {
            return Err(to_js(Bad(format!(
                "the fetch left no complete blob: {other:?}"
            ))));
        }
        Err(error) => {
            return Err(to_js(Bad(format!(
                "the stored blob cannot be read: {error}"
            ))));
        }
    };
    if stored != size {
        return Err(to_js(Bad(format!(
            "the content is {stored} bytes, and the ticket names {size}"
        ))));
    }
    let bytes = store
        .blobs()
        .get_bytes(hash)
        .await
        .map_err(|e| to_js(Bad(format!("the blob cannot be read: {e}"))))?;
    Ok(bytes.to_vec())
}

/// A byte count a page passes as a number. A count that is not a whole number of bytes in the range
/// a `u64` holds is refused, because the size a ticket names is one.
fn byte_count(size: f64) -> Result<u64, Bad> {
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size > u64::MAX as f64 {
        return Err(Bad("a ticket's size is a whole number of bytes".to_string()));
    }
    Ok(size as u64)
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
