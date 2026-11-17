//! The spike's exports: a handshake that says which key exchange it agreed on, and a fetch that says
//! how long the download took.
//!
//! `pq.rs` is the group and the provider; this module is what a page or the Node runner calls. It is
//! separate from `lib.rs` because the spike is not the crate's API: `fetch_blob` there is the product's
//! fetch, and everything here exists to answer one question with numbers.

use std::sync::atomic::{AtomicU64, Ordering};

use iroh::{Endpoint, EndpointAddr, address_lookup::MemoryLookup, endpoint::presets};
use iroh_blobs::Hash;
use wasm_bindgen::prelude::*;

use crate::pq;
use crate::{Bad, RELAY_WAIT, byte_count, hash, relay, relay_only, sender_id, to_js};

/// The ALPN the spike's native peer answers a handshake on. It is not a Sluice protocol: nothing but
/// a measurement uses it.
const SPIKE_ALPN: &[u8] = b"spike/pq/1";

/// How long the last [`fetch_blob_pq`] spent on the download itself, in microseconds, and not on the
/// endpoint or the relay.
static LAST_FETCH_MICROS: AtomicU64 = AtomicU64::new(0);

/// The same fetch with a key exchange the caller names, for the spike: `classical` or `hybrid`.
///
/// The download itself is timed, and [`pq_last_fetch_ms`] answers how long it took: the endpoint,
/// the relay and the tab's own frame are outside the number, because a rate that includes starting a
/// relay connection measures the relay and not the transfer.
///
/// # Errors
///
/// Everything `fetch_blob` refuses, and a mode that is neither `classical` nor `hybrid`.
#[wasm_bindgen]
pub async fn fetch_blob_pq(
    relay_url: &str,
    sender: &str,
    root: &str,
    size: f64,
    mode: &str,
) -> Result<Vec<u8>, JsValue> {
    let mode = pq::Mode::parse(mode)
        .ok_or_else(|| to_js(Bad("a mode is classical or hybrid".to_string())))?;
    let url = relay(relay_url).map_err(to_js)?;
    let sender = sender_id(sender).map_err(to_js)?;
    let hash = hash(root).map_err(to_js)?;
    let size = byte_count(size).map_err(to_js)?;
    // A ticket of no bytes names the root of empty content, and there is nothing to ask a peer for:
    // the check is the whole fetch, and it still refuses a root that is not the one named.
    if size == 0 {
        if hash != Hash::new(b"") {
            return Err(to_js(Bad(
                "the content is 0 bytes, and the root is not the empty content's".to_string(),
            )));
        }
        return Ok(Vec::new());
    }
    // The only address a tab can use is the relay, and the ticket already says which one the sender
    // is on. Without this, a browser has an id and no way to dial it.
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(EndpointAddr::new(sender).with_relay_url(url.clone()));
    let endpoint = Endpoint::builder(presets::Minimal)
        .crypto_provider(pq::provider(mode, pq::Probe::new()))
        .relay_mode(relay_only(url))
        .address_lookup(lookup)
        .bind()
        .await
        .map_err(|e| to_js(Bad(format!("cannot open an endpoint: {e}"))))?;
    // The relay first, then the download, so the number is the transfer and not the rendezvous.
    let online = n0_future::time::timeout(RELAY_WAIT, endpoint.online());
    let outcome = if online.await.is_err() {
        Err(Bad(format!(
            "the relay did not answer in {} s",
            RELAY_WAIT.as_secs()
        )))
    } else {
        let started = now_ms();
        let fetched = crate::fetch::fetch_direct(&endpoint, sender, hash, size).await;
        let micros = (now_ms() - started).max(0.0) * 1000.0;
        LAST_FETCH_MICROS.store(micros as u64, Ordering::SeqCst);
        fetched
    };
    let outcome = match outcome {
        Ok(bytes) => Ok(bytes),
        Err(missing) => Err(to_js(missing)),
    };
    // Closed on every way out: a page that keeps an endpoint open holds a socket and a relay
    // registration for as long as it lives.
    endpoint.close().await;
    outcome
}

/// How many milliseconds the last [`fetch_blob_pq`] spent on the download, after the relay answered
/// and before the endpoint closed.
#[wasm_bindgen]
pub fn pq_last_fetch_ms() -> f64 {
    LAST_FETCH_MICROS.load(Ordering::SeqCst) as f64 / 1000.0
}

/// One handshake with the peer `sender` through `relay_url`, on the spike's own ALPN, offering the
/// key exchange `mode` names.
///
/// Answers one JSON object: `mode`, `group`, which is the group that actually completed, and
/// `handshake_ms`, which is the time `connect` took and nothing else. The connection is answered by
/// a peer that reads one byte and writes one back, so a handshake that returns is a connection that
/// both sides have.
///
/// # Errors
///
/// A relay URL, an id or a mode that is not one, a relay that does not answer in time, a handshake
/// that fails, and a peer that does not answer on the spike's ALPN are strings for the caller to
/// show.
#[wasm_bindgen]
pub async fn pq_handshake(relay_url: &str, sender: &str, mode: &str) -> Result<String, JsValue> {
    let url = relay(relay_url).map_err(to_js)?;
    let sender = sender_id(sender).map_err(to_js)?;
    let mode = pq::Mode::parse(mode)
        .ok_or_else(|| to_js(Bad("a mode is classical or hybrid".to_string())))?;
    let probe = pq::Probe::new();
    let lookup = MemoryLookup::new();
    lookup.add_endpoint_info(EndpointAddr::new(sender).with_relay_url(url.clone()));
    let endpoint = Endpoint::builder(presets::Minimal)
        .crypto_provider(pq::provider(mode, probe))
        .relay_mode(relay_only(url.clone()))
        .address_lookup(lookup)
        .bind()
        .await
        .map_err(|e| to_js(Bad(format!("cannot open an endpoint: {e}"))))?;
    let online = n0_future::time::timeout(RELAY_WAIT, endpoint.online());
    if online.await.is_err() {
        endpoint.close().await;
        return Err(to_js(Bad(format!(
            "the relay did not answer in {} s",
            RELAY_WAIT.as_secs()
        ))));
    }
    let address = EndpointAddr::new(sender).with_relay_url(url);
    let started = now_ms();
    let connected = endpoint.connect(address, SPIKE_ALPN).await;
    let elapsed = now_ms() - started;
    let result = match connected {
        Ok(connection) => {
            // One byte out and one back, so the peer has seen the connection and the harness knows
            // the handshake was not the only thing that worked.
            let exchanged = async {
                let (mut send, mut recv) = connection.open_bi().await.map_err(|e| e.to_string())?;
                send.write_all(b"pq").await.map_err(|e| e.to_string())?;
                send.finish().map_err(|e| e.to_string())?;
                let mut reply = [0u8; 2];
                recv.read_exact(&mut reply)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok::<[u8; 2], String>(reply)
            }
            .await;
            connection.close(0u32.into(), b"spike done");
            match exchanged {
                Ok(reply) => Ok(format!(
                    "{{\"mode\":\"{}\",\"group\":\"{}\",\"handshake_ms\":{:.3},\"reply\":\"{}\"}}",
                    mode.name(),
                    probe.agreed().unwrap_or("none"),
                    elapsed,
                    String::from_utf8_lossy(&reply),
                )),
                Err(error) => Err(to_js(Bad(format!("the peer did not answer: {error}")))),
            }
        }
        Err(error) => Err(to_js(Bad(format!("the handshake failed: {error}")))),
    };
    endpoint.close().await;
    result
}

/// Milliseconds on a clock that runs, for a measurement the tab reports.
#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    let global = js_sys::global();
    let now = js_sys::Reflect::get(&global, &JsValue::from_str("performance"))
        .ok()
        .and_then(|performance| {
            let now = js_sys::Reflect::get(&performance, &JsValue::from_str("now")).ok()?;
            let now = now.dyn_into::<js_sys::Function>().ok()?;
            now.call0(&performance).ok()?.as_f64()
        });
    now.unwrap_or_else(js_sys::Date::now)
}

/// The same clock on the host, where a test runs.
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}
