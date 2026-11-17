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

mod fetch;
pub mod pq;
mod spike;

use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl,
    address_lookup::MemoryLookup, endpoint::presets,
};
use iroh_blobs::{Hash, store::mem::MemStore};
use wasm_bindgen::prelude::*;

/// How long a tab waits for its relay before it gives up. A relay that has not answered in this
/// time is not going to carry the transfer, and a page should show a failure rather than wait.
const RELAY_WAIT: Duration = Duration::from_secs(20);

/// How long a fetch goes without the store growing before it gives up. A peer that answers the relay
/// and then stops sending would otherwise leave the promise pending and the endpoint open for as
/// long as the tab lives, while a file of a few hundred megabytes over a slow relay is worth waiting
/// for as long as it keeps arriving: the limit is the progress, not the whole fetch.
const IDLE_WAIT: Duration = Duration::from_secs(20);

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
        // The largest count a `u64` holds is not a `f64`, so the bound is the first number above
        // it: a size of `2^64` is refused rather than turned into a count of its own.
        assert!(byte_count(u64::MAX as f64).is_err(), "2^64");
        assert!(byte_count((u64::MAX / 2) as f64).is_ok());
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

    /// A lookup that knows one address, which is what a tab gets from its relay.
    #[cfg(not(target_arch = "wasm32"))]
    fn lookup_of(addr: iroh::EndpointAddr) -> iroh::address_lookup::MemoryLookup {
        let lookup = iroh::address_lookup::MemoryLookup::new();
        lookup.add_endpoint_info(addr);
        lookup
    }

    /// A ticket that names fewer bytes than the content holds fetches only the bytes it names and
    /// then refuses the content, so a ticket cannot make a tab hold a file it was not told about.
    /// This is a host test: a tab's fetch needs a relay, and only the host build carries an async
    /// runtime to drive one.
    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn a_ticket_that_names_fewer_bytes_than_the_content_is_refused() {
        use iroh::{Endpoint, protocol::Router};
        use iroh_blobs::BlobsProtocol;
        use iroh_blobs::store::mem::MemStore;

        use super::fetch::fetch_direct;

        let content = vec![7u8; 1 << 20];
        let store = MemStore::new();
        let hash = store.blobs().add_bytes(content.clone()).await.unwrap().hash;
        let provider = Endpoint::builder(iroh::endpoint::presets::Empty)
            .crypto_provider(iroh::tls::default_provider())
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        let router = Router::builder(provider.clone())
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&store, None))
            .spawn();
        let consumer = Endpoint::builder(iroh::endpoint::presets::Empty)
            .crypto_provider(iroh::tls::default_provider())
            .relay_mode(iroh::RelayMode::Disabled)
            // A tab learns the sender's address from the relay it is told about; here the provider
            // is on this machine and says where it is.
            .address_lookup(lookup_of(provider.addr()))
            .bind()
            .await
            .unwrap();

        // A thousand bytes of the megabyte: the fetch asks for that range and no more, so the store
        // is left incomplete rather than holding the megabyte and being refused afterwards.
        let allowed = 1024u64;
        let error = fetch_direct(&consumer, provider.id(), hash, allowed)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            format!("the content is longer than the {allowed} bytes the ticket names")
        );

        // The whole content, asked for in full, still arrives: the bound is the ticket's length and
        // not a smaller fetch.
        let bytes = fetch_direct(&consumer, provider.id(), hash, content.len() as u64)
            .await
            .unwrap();
        assert_eq!(bytes, content);

        router.shutdown().await.unwrap();
        consumer.close().await;
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
/// `size` is the length the ticket promises, as a number, and it is how much the fetch asks for: the
/// download requests the bytes below that length and no others, so a ticket that names a kilobyte
/// for a root of ten gigabytes costs the tab a kilobyte of transfer and of memory, and the content
/// is refused afterwards because it is not the length the ticket names. The relay has
/// [`RELAY_WAIT`] to answer, and the fetch then has [`IDLE_WAIT`] without the store growing before
/// it gives up; the endpoint is closed on every way out, including a refusal and a timeout.
///
/// It has fetched from a native `sluice send` through a relay, in a headless browser: a 2 MiB file
/// whose SHA-256 matched the sender's. The address it needs is the ticket's relay, which names where
/// the sender is, because a tab can reach nothing directly. `docs/browser.md` has the run.
///
/// # Errors
///
/// A relay URL, a sender id, a root or a size that is not one, a relay that does not answer in time,
/// a fetch that does not finish in time, an endpoint that cannot be bound, a fetch that fails, and
/// content of another length are all strings for the caller to show. None of them quotes the
/// ticket.
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
    // A ticket of no bytes names the root of empty content, and there is nothing to ask a peer for:
    // the check is the whole fetch, and it still refuses a root that is not that one.
    if size == 0 {
        if hash != Hash::new(b"") {
            return Err(to_js(Bad(
                "the content is 0 bytes, and the root is not the empty content's".to_string(),
            )));
        }
        return Ok(Vec::new());
    }
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
    // The whole fetch is bounded, not only the relay's answer: a peer that stops sending fails the
    // promise instead of leaving the endpoint open for the life of the tab.
    // The fetch is bounded by its progress rather than by a total, so a large file on a slow relay
    // finishes; the endpoint is closed on every way out, a timeout included.
    let outcome = match fetch::fetch(&endpoint, sender, hash, size).await {
        Ok(bytes) => Ok(bytes),
        Err(missing) => Err(to_js(missing)),
    };
    // Closed on every way out: a page that keeps an endpoint open holds a socket and a relay
    // registration for as long as it lives.
    endpoint.close().await;
    outcome
}

/// A byte count a page passes as a number. A count that is not a whole number of bytes in the range
/// a `u64` holds is refused, because the size a ticket names is one. `2^64` is refused too: `u64`
/// holds nothing above `2^64 - 1`, and rounding that bound up to a `f64` would turn a size of `2^64`
/// into a count of its own.
pub(crate) fn byte_count(size: f64) -> Result<u64, Bad> {
    if !size.is_finite() || size < 0.0 || size.fract() != 0.0 || size >= u64::MAX as f64 {
        return Err(Bad("a ticket's size is a whole number of bytes".to_string()));
    }
    Ok(size as u64)
}

/// The endpoint id a ticket names, from its 64 hex characters.
pub(crate) fn sender_id(text: &str) -> Result<EndpointId, Bad> {
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
pub(crate) fn to_js(error: Bad) -> JsValue {
    JsValue::from_str(&error.0)
}
