#![allow(clippy::unwrap_used)]
//! `/meet` on the same listener as `/relay`: the upgrade, claim, join, a third connection, a burn,
//! and the 404 when the config has no meeting place.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use sluice_meet::frame::{ClientFrame, ErrorCode, FailReason, Role, ServerFrame};
use sluice_meet::meet::Config as MeetLimits;
use sluice_relay::config::Config;
use sluice_relay::meet::MeetHttp;
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, MaybeTlsStream, Message, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PLAIN: &str = "enable_metrics = false\nhttp_bind_addr = \"127.0.0.1:0\"\n\
                     [access]\nallowlist = []\n";

fn plain(meet: bool) -> Config {
    let text = if meet {
        format!(
            "{PLAIN}[meet]\nenable = true\nrate_per_ip = 100\nmax_plates = 10\nttl_seconds = 600\n"
        )
    } else {
        PLAIN.to_string()
    };
    Config::parse(&text).unwrap()
}

async fn start(meet: bool) -> (sluice_relay::server::Running, Option<Arc<MeetHttp>>) {
    let config = plain(meet);
    let http = if meet {
        Some(MeetHttp::new(MeetLimits::new(100, 10, 600).unwrap()))
    } else {
        None
    };
    let running = sluice_relay::server::start(config, http.clone())
        .await
        .unwrap();
    (running, http)
}

async fn connect(addr: std::net::SocketAddr) -> Ws {
    ClientBuilder::from_uri(format!("ws://{addr}/meet").parse().unwrap())
        .connect()
        .await
        .expect("a WebSocket to /meet")
        .0
}

async fn send(ws: &mut Ws, frame: ClientFrame) {
    ws.send(Message::binary(frame.encode().unwrap()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerFrame {
    loop {
        let message = ws.next().await.expect("a message").expect("a frame");
        if message.is_binary() {
            return ServerFrame::decode(message.into_payload().as_ref()).expect("a server frame");
        }
    }
}

#[tokio::test]
async fn claim_join_and_a_third_connection_are_the_meeting_place() {
    let (running, _http) = start(true).await;
    let mut sender = connect(running.addr).await;
    send(&mut sender, ClientFrame::Claim).await;
    assert_eq!(recv(&mut sender).await, ServerFrame::Claimed(1));

    let mut receiver = connect(running.addr).await;
    send(&mut receiver, ClientFrame::Join(1)).await;
    assert!(matches!(
        recv(&mut sender).await,
        ServerFrame::Joined(Role::Sender)
    ));
    assert!(matches!(
        recv(&mut receiver).await,
        ServerFrame::Joined(Role::Receiver)
    ));

    let mut third = connect(running.addr).await;
    send(&mut third, ClientFrame::Join(1)).await;
    assert_eq!(recv(&mut third).await, ServerFrame::Error(ErrorCode::Busy));
    running.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_confirmation_burns_both_sides() {
    let (running, _http) = start(true).await;
    let mut sender = connect(running.addr).await;
    send(&mut sender, ClientFrame::Claim).await;
    let _ = recv(&mut sender).await;
    let mut receiver = connect(running.addr).await;
    send(&mut receiver, ClientFrame::Join(1)).await;
    let _ = recv(&mut sender).await;
    let _ = recv(&mut receiver).await;

    send(&mut sender, ClientFrame::Fail(FailReason::ConfirmMismatch)).await;
    assert_eq!(recv(&mut sender).await, ServerFrame::Burned);
    assert_eq!(recv(&mut receiver).await, ServerFrame::Burned);
    running.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_trace_log_holds_no_nameplate_or_sealed_value() {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Sink {
        type Writer = Sink;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    let captured = Arc::new(Mutex::new(Vec::new()));
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(Sink(captured.clone()))
        .try_init();

    let (running, _http) = start(true).await;
    let mut sender = connect(running.addr).await;
    send(&mut sender, ClientFrame::Claim).await;
    let _ = recv(&mut sender).await;
    let mut receiver = connect(running.addr).await;
    send(&mut receiver, ClientFrame::Join(1)).await;
    let _ = recv(&mut sender).await;
    let _ = recv(&mut receiver).await;
    send(&mut sender, ClientFrame::Sealed(vec![b'X'; 28])).await;
    assert_eq!(
        recv(&mut receiver).await,
        ServerFrame::Sealed(vec![b'X'; 28])
    );
    running.shutdown().await.unwrap();

    let text = String::from_utf8_lossy(&captured.lock().unwrap()).to_string();
    for secret in ["sluice1", "orbit", "velvet", "XXXXXXXX", "nameplate"] {
        assert!(!text.contains(secret), "the log holds {secret}: {text}");
    }
}

#[tokio::test]
async fn without_meet_the_path_is_not_found() {
    let (running, _http) = start(false).await;
    let answer = reqwest::get(format!("http://{}/meet", running.addr))
        .await
        .unwrap();
    assert_eq!(answer.status(), reqwest::StatusCode::NOT_FOUND);
    running.shutdown().await.unwrap();
}
