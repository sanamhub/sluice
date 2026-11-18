//! `GET /meet` on the relay's listener: the WebSocket upgrade, the meeting-place state machine,
//! and the per-connection frame loop.
//!
//! Nothing here logs a code, nameplate, ticket, SPAKE2 message or sealed value. A refusal is logged
//! by its [`ErrorCode`] alone, and the state machine itself never formats a frame.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use base64::Engine;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http::header::{CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, UPGRADE};
use http::response::Builder as ResponseBuilder;
use http::{Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper_util::rt::TokioIo;
use iroh_relay::server::http_server::{BytesBody, HyperResult};
use sluice_meet::frame::{ClientFrame, ErrorCode, MAX_FRAME, ServerFrame};
use sluice_meet::meet::{Config as MeetLimits, ConnectionId, MeetingPlace, Outbound};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_websockets::{Message, ServerBuilder, WebSocketStream};

tokio::task_local! {
    /// The peer address of the connection whose request is being handled. `RelayService` does not
    /// pass it to a `Handlers` closure, so the accept loop scopes it around the connection.
    pub static PEER_IP: IpAddr;
}

/// The meeting place as the HTTP layer holds it.
#[derive(Clone)]
pub struct MeetHttp {
    place: Arc<AsyncMutex<MeetingPlace>>,
    peers: Arc<Mutex<HashMap<ConnectionId, UnboundedSender<ServerFrame>>>>,
    enabled: bool,
}

impl std::fmt::Debug for MeetHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeetHttp")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl MeetHttp {
    /// A meeting place with the guard-checked limits, and the timer that expires old plates.
    pub fn new(limits: MeetLimits) -> Arc<Self> {
        let place = Arc::new(AsyncMutex::new(MeetingPlace::new(limits)));
        let peers = Arc::new(Mutex::new(HashMap::new()));
        let this = Arc::new(Self {
            place: place.clone(),
            peers: peers.clone(),
            enabled: true,
        });
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let out = place.lock().await.expire(now_ms());
                deliver(&peers, out);
            }
        });
        this
    }

    /// The `Handlers` closure for `/meet`. It answers a WebSocket upgrade and leaves the frame
    /// loop running on its own task.
    pub fn handler(
        &self,
    ) -> impl Fn(Request<Incoming>, ResponseBuilder) -> HyperResult<Response<BytesBody>>
    + Send
    + Sync
    + 'static {
        let this = self.clone();
        move |request, _builder| this.upgrade(request)
    }

    fn upgrade(&self, request: Request<Incoming>) -> HyperResult<Response<BytesBody>> {
        if !self.enabled {
            return Ok(response(StatusCode::NOT_FOUND, "not found"));
        }
        let Some(key) = request.headers().get(SEC_WEBSOCKET_KEY).cloned() else {
            return Ok(response(StatusCode::BAD_REQUEST, "expected a WebSocket"));
        };
        let accept = accept_key(key.as_bytes());
        let ip = PEER_IP
            .try_with(|ip| *ip)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let this = self.clone();
        tokio::spawn(async move {
            if let Ok(upgraded) = hyper::upgrade::on(request).await {
                let ws = ServerBuilder::new().serve(TokioIo::new(upgraded));
                this.connection(ws, ip).await;
            }
        });
        Ok(Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS)
            .header(UPGRADE, "websocket")
            .header(CONNECTION, "Upgrade")
            .header(SEC_WEBSOCKET_ACCEPT, accept)
            .body(empty())
            .expect("a fixed response"))
    }

    async fn connection<S>(&self, mut ws: WebSocketStream<S>, ip: IpAddr)
    where
        S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    {
        let id = {
            let mut place = self.place.lock().await;
            match place.connect(ip, now_ms()) {
                Ok(id) => id,
                Err(code) => {
                    let _ = send(&mut ws, ServerFrame::Error(code)).await;
                    tracing::warn!(%code, "meeting place refused a connection");
                    return;
                }
            }
        };
        let (own_tx, mut own_rx): (UnboundedSender<ServerFrame>, UnboundedReceiver<ServerFrame>) =
            unbounded_channel();
        self.peers
            .lock()
            .expect("the peer table")
            .insert(id, own_tx.clone());

        loop {
            tokio::select! {
                incoming = ws.next() => {
                    let Some(Ok(message)) = incoming else { break };
                    if message.is_close() {
                        break;
                    }
                    if message.is_text() {
                        let _ = send(&mut ws, ServerFrame::Error(ErrorCode::BadFrame)).await;
                        break;
                    }
                    let bytes = message.into_payload();
                    if bytes.len() > MAX_FRAME {
                        let _ = send(&mut ws, ServerFrame::Error(ErrorCode::TooLarge)).await;
                        break;
                    }
                    let Ok(frame) = ClientFrame::decode(bytes.as_ref()) else {
                        let _ = send(&mut ws, ServerFrame::Error(ErrorCode::BadFrame)).await;
                        break;
                    };
                    let out = {
                        let mut place = self.place.lock().await;
                        place.receive(id, frame, now_ms())
                    };
                    match out {
                        Ok(out) => deliver(&self.peers, out),
                        Err(code) => {
                            let _ = send(&mut ws, ServerFrame::Error(code)).await;
                            tracing::warn!(%code, "meeting place refused a frame");
                            break;
                        }
                    }
                }
                Some(frame) = own_rx.recv() => {
                    if send(&mut ws, frame).await.is_err() {
                        break;
                    }
                }
            }
        }

        self.peers.lock().expect("the peer table").remove(&id);
        let out = self.place.lock().await.disconnect(id, now_ms());
        deliver(&self.peers, out);
    }
}

/// Sends every returned frame to the connection it names. `own` is the connection doing the send,
/// whose frames are already on its own channel.
fn deliver(peers: &Mutex<HashMap<ConnectionId, UnboundedSender<ServerFrame>>>, out: Vec<Outbound>) {
    if out.is_empty() {
        return;
    }
    let peers = peers.lock().expect("the peer table");
    for Outbound { to, frame } in out {
        if let Some(tx) = peers.get(&to) {
            let _ = tx.send(frame);
        }
    }
}

async fn send<S>(ws: &mut WebSocketStream<S>, frame: ServerFrame) -> Result<(), ()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    ws.send(Message::binary(frame.encode().map_err(|_| ())?))
        .await
        .map_err(|_| ())
}

fn now_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn empty() -> BytesBody {
    Box::new(Full::new(Bytes::new()))
}

fn response(status: StatusCode, text: &'static str) -> Response<BytesBody> {
    let body: BytesBody = Box::new(Full::new(Bytes::from_static(text.as_bytes())));
    Response::builder()
        .status(status)
        .body(body)
        .expect("a fixed response")
}

fn accept_key(key: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut sha1 = Sha1::new();
    sha1.update(key);
    sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha1.finalize())
}
