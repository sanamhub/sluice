//! The short-code command line against the meeting-place library on loopback: the two exact
//! refusals, a right code that gives the ticket back, and a wrong one that burns it.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use futures_util::{SinkExt, StreamExt};
use sluice_cli::short_code;
use sluice_core::Error;
use sluice_meet::Code;
use sluice_meet::frame::{ClientFrame, ServerFrame};
use sluice_meet::meet::{Config as MeetLimits, ConnectionId, MeetingPlace};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_websockets::{Message, ServerBuilder, WebSocketStream};

use crate::common::{sluice, stderr_text};

fn home(dir: &Path) -> std::path::PathBuf {
    dir.join("home")
}

#[test]
fn send_code_without_a_relay_is_the_exact_policy_message() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.bin");
    std::fs::write(&file, b"a").unwrap();
    let failed = sluice(&home(dir.path()))
        .args(["send", "--code", file.to_str().unwrap()])
        .assert()
        .code(6);
    assert_eq!(
        stderr_text(&failed),
        "sluice: policy denied: --code needs a relay with a meeting place, and no relay is \
         configured; set --relay or send without --code\n"
    );
}

#[test]
fn recv_a_code_without_a_relay_says_which_flag_names_it() {
    let dir = tempfile::tempdir().unwrap();
    let failed = sluice(&home(dir.path()))
        .args(["recv", "7-orbit-velvet"])
        .assert()
        .code(6);
    let text = stderr_text(&failed);
    assert!(
        text.contains("a code needs the relay the sender used: pass --relay URL"),
        "{text}"
    );
}

type Ws = WebSocketStream<TcpStream>;

async fn start_meeting() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let place = Arc::new(Mutex::new(MeetingPlace::new(
        MeetLimits::new(100, 10, 600).unwrap(),
    )));
    let peers: Arc<Mutex<HashMap<ConnectionId, tokio::sync::mpsc::UnboundedSender<ServerFrame>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                return;
            };
            let place = place.clone();
            let peers = peers.clone();
            tokio::spawn(async move {
                let Ok((_, mut ws)) = ServerBuilder::new().accept(stream).await else {
                    return;
                };
                let id = {
                    let mut place = place.lock().await;
                    place.connect(peer.ip(), now()).unwrap()
                };
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                peers.lock().await.insert(id, tx);
                loop {
                    tokio::select! {
                        incoming = ws.next() => {
                            let Some(Ok(message)) = incoming else { break };
                            if message.is_close() {
                                break;
                            }
                            let Ok(frame) = ClientFrame::decode(message.as_payload()) else { break };
                            let out = {
                                let mut place = place.lock().await;
                                place.receive(id, frame, now())
                            };
                            let out = match out {
                                Ok(out) => out,
                                Err(code) => {
                                    let _ = send(&mut ws, ServerFrame::Error(code)).await;
                                    break;
                                }
                            };
                            deliver(&peers, out).await;
                        }
                        Some(frame) = rx.recv() => {
                            if send(&mut ws, frame).await.is_err() {
                                break;
                            }
                        }
                    }
                }
                peers.lock().await.remove(&id);
                let out = place.lock().await.disconnect(id, now());
                deliver(&peers, out).await;
            });
        }
    });
    (addr, task)
}

async fn deliver(
    peers: &Mutex<HashMap<ConnectionId, tokio::sync::mpsc::UnboundedSender<ServerFrame>>>,
    out: Vec<sluice_meet::meet::Outbound>,
) {
    let peers = peers.lock().await;
    for one in out {
        if let Some(tx) = peers.get(&one.to) {
            let _ = tx.send(one.frame);
        }
    }
}

async fn send(ws: &mut Ws, frame: ServerFrame) -> Result<(), ()> {
    ws.send(Message::binary(frame.encode().map_err(|_| ())?))
        .await
        .map_err(|_| ())
}

fn now() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_right_words_give_the_ticket_back() {
    let (addr, server) = start_meeting().await;
    let relay = format!("http://{addr}");
    let ticket = "sluice1abcdefghjkmnpqrstvwxyz";
    let (ws, sender, code) = short_code::claim(&relay, ticket).await.unwrap();
    let driving = tokio::spawn(short_code::drive_sender(ws, sender));
    let got = short_code::resolve(&relay, Code::parse(&code).unwrap())
        .await
        .unwrap();
    assert_eq!(got, ticket);
    driving.abort();
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wrong_word_pair_burns_the_code() {
    let (addr, server) = start_meeting().await;
    let relay = format!("http://{addr}");
    let (ws, sender, code) = short_code::claim(&relay, "sluice1abc").await.unwrap();
    let driving = tokio::spawn(short_code::drive_sender(ws, sender));
    let plate = code.split('-').next().unwrap();
    let wrong = Code::parse(&format!("{plate}-abandon-zoo")).unwrap();
    let error = short_code::resolve(&relay, wrong).await.unwrap_err();
    assert!(error.to_string().contains("burned"), "{error}");
    driving.abort();
    server.abort();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_without_meet_is_the_exact_message() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut buffer = [0u8; 4096];
        let _ = stream.read(&mut buffer).await;
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await;
    });
    let error = short_code::claim(&format!("http://{addr}"), "sluice1abc")
        .await
        .unwrap_err();
    assert!(
        matches!(&error, Error::Policy(text) if text == short_code::NO_MEET),
        "{error}"
    );
    server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_connection_failure_is_a_network_error() {
    let error = short_code::claim("http://127.0.0.1:1", "sluice1abc")
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Network(_)), "{error}");
}
