//! `sluice daemon --ui`: the page, and the control interface as JSON, over HTTP on a loopback port.
//! ADR 0036 says what guards it: the listener binds `127.0.0.1` and refuses any other peer, every
//! API call carries a 32-byte random key, a request is for the page's own `Host` and `Origin`, no
//! answer has a CORS header, a POST is JSON of at most 64 KiB, and the page is static and strict.
//! The page never carries file bytes: it asks the daemon, which is a peer, to send and receive.

mod assets;
mod guard;
mod json;
mod qr;
mod routes;

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use sluice_api::Service;
use sluice_core::Error;
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio::task::JoinHandle;

/// The file in the home that holds the port and the key, for `sluice ui`.
pub const PORT_FILE: &str = "ui.json";

/// How many connections the listener serves at once.
const MAX_CONNECTIONS: usize = 64;

/// How long a connection has to send the head of a request.
const HEAD_TIMEOUT: Duration = Duration::from_secs(10);

/// What an answer's body is.
pub type Body = BoxBody<Bytes, std::io::Error>;

/// What every handler needs to know.
pub struct State {
    pub service: Service,
    pub key: String,
    pub port: u16,
}

/// A running listener.
pub struct Page {
    port: u16,
    key: String,
    task: JoinHandle<()>,
}

impl Page {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// What `sluice ui` reads: the port and the key, in one small object.
    pub fn file_text(&self) -> String {
        serde_json::json!({ "port": self.port, "key": self.key, "pid": std::process::id() })
            .to_string()
    }

    /// Stops accepting. A connection that is open ends with the runtime.
    pub fn stop(self) {
        self.task.abort();
    }
}

/// 32 random bytes as 64 hex characters.
pub fn make_key() -> Result<String, Error> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|_| Error::Storage("the system has no random bytes to give".to_string()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Binds `127.0.0.1` on a port the system picks and starts serving.
pub async fn start(service: Service) -> Result<Page, Error> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| Error::Network(format!("cannot listen on the loopback address: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::Network(format!("cannot read the port: {e}")))?
        .port();
    let key = make_key()?;
    let state = Arc::new(State {
        service,
        key: key.clone(),
        port,
    });
    let task = tokio::spawn(accept_loop(listener, state));
    Ok(Page { port, key, task })
}

async fn accept_loop(listener: TcpListener, state: Arc<State>) {
    let places = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        let Ok((stream, peer)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        if !allowed_peer(peer) {
            continue;
        }
        let Ok(place) = places.clone().acquire_owned().await else {
            return;
        };
        let state = state.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| serve(state.clone(), request));
            let served = http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(HEAD_TIMEOUT)
                .serve_connection(TokioIo::new(stream), service)
                .await;
            // A client that goes away mid-answer is not news.
            drop(served);
            drop(place);
        });
    }
}

/// Whether a connection from `peer` is served: only a loopback peer is, and any other is closed.
fn allowed_peer(peer: SocketAddr) -> bool {
    guard::is_loopback_peer(peer)
}

async fn serve(
    state: Arc<State>,
    request: Request<Incoming>,
) -> Result<Response<Body>, Infallible> {
    Ok(routes::route(&state, request)
        .await
        .unwrap_or_else(|reject| error(reject.status, reject.message)))
}

/// An answer with the headers every answer has.
pub fn respond(status: u16, content_type: &str, body: Body) -> Response<Body> {
    let mut response = Response::new(body);
    *response.status_mut() =
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let headers = response.headers_mut();
    if let Ok(value) = content_type.parse() {
        headers.insert("content-type", value);
    }
    for (name, value) in assets::HEADERS {
        headers.insert(name, value.parse().expect("a fixed header value"));
    }
    response
}

pub fn full(bytes: impl Into<Bytes>) -> Body {
    Full::new(bytes.into())
        .map_err(|never| match never {})
        .boxed()
}

/// `{"error": "<sentence>"}` with a status.
pub fn error(status: u16, message: &str) -> Response<Body> {
    let body = serde_json::json!({ "error": message }).to_string();
    respond(status, "application/json", full(body))
}

#[cfg(test)]
mod tests;
