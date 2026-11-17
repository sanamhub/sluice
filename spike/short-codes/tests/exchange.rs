//! The three exchanges the spike has to show, plus the relay and the meeting place on one port.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use iroh_base::SecretKey;
use iroh_dns::dns::DnsResolver;
use iroh_relay::client::ClientBuilder;
use iroh_relay::protos::relay::{ClientToRelayMsg, Datagrams, RelayToClientMsg};
use iroh_relay::tls::CaTlsConfig;
use short_codes_spike::code::Code;
use short_codes_spike::crypto::Started;
use short_codes_spike::meet::{Config, MeetingPlace};
use short_codes_spike::server::{self, client_config};
use short_codes_spike::wire::{ClientMsg, ErrorCode, Role, ServerMsg};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_websockets::{ClientBuilder as WsBuilder, Connector, MaybeTlsStream, WebSocketStream};
use url::Url;

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

async fn start(
    place: Arc<MeetingPlace>,
    tls: bool,
) -> (
    std::net::SocketAddr,
    Option<rustls::pki_types::CertificateDer<'static>>,
) {
    if tls {
        let (acceptor, cert) = server::self_signed().unwrap();
        let addr = server::spawn(([127, 0, 0, 1], 0).into(), place, Some(acceptor))
            .await
            .unwrap();
        (addr, Some(cert))
    } else {
        let addr = server::spawn(([127, 0, 0, 1], 0).into(), place, None)
            .await
            .unwrap();
        (addr, None)
    }
}

async fn connect(
    addr: std::net::SocketAddr,
    cert: Option<&rustls::pki_types::CertificateDer<'static>>,
) -> Ws {
    let scheme = if cert.is_some() { "wss" } else { "ws" };
    let uri = format!("{scheme}://127.0.0.1:{}/meet", addr.port());
    match cert {
        Some(cert) => {
            let config = client_config(cert);
            let connector = Connector::Rustls(TlsConnector::from(config));
            WsBuilder::from_uri(uri.parse().unwrap())
                .connector(&connector)
                .connect()
                .await
                .unwrap()
                .0
        }
        None => {
            WsBuilder::from_uri(uri.parse().unwrap())
                .connect()
                .await
                .unwrap()
                .0
        }
    }
}

async fn send(ws: &mut Ws, msg: ClientMsg) {
    ws.send(tokio_websockets::Message::binary(msg.encode()))
        .await
        .unwrap();
}

async fn recv(ws: &mut Ws) -> ServerMsg {
    loop {
        let msg = ws.next().await.expect("a frame").expect("a frame");
        assert!(!msg.is_close(), "the meeting place closed the connection");
        if msg.is_binary() {
            return ServerMsg::decode(&msg.into_payload()).expect("a known frame");
        }
    }
}

/// The sender claims, then runs its half. It returns the plate and reports whether the exchange
/// finished with `Done` or with `Burned`.
async fn sender(
    addr: std::net::SocketAddr,
    tls_cert: Option<rustls::pki_types::CertificateDer<'static>>,
    ticket: &'static str,
    plate_tx: tokio::sync::oneshot::Sender<u32>,
) -> (u32, Result<(), ()>) {
    let mut ws = connect(addr, tls_cert.as_ref()).await;
    send(&mut ws, ClientMsg::Claim).await;
    let plate = match recv(&mut ws).await {
        ServerMsg::Claimed(p) => p,
        other => panic!("expected Claimed, got {other:?}"),
    };
    // "orbit" and "velvet" are in the BIP-39 list, and the code carries the server's plate.
    let code = Code::parse(&format!("{plate}-orbit-velvet")).unwrap();
    let _ = plate_tx.send(plate);
    let started = Started::new(Role::Sender, &code);
    assert!(matches!(
        recv(&mut ws).await,
        ServerMsg::Joined(Role::Sender)
    ));
    send(&mut ws, ClientMsg::Spake(started.outbound.clone())).await;
    let peer = match recv(&mut ws).await {
        ServerMsg::Spake(m) => m,
        other => panic!("expected Spake, got {other:?}"),
    };
    let keys = started.finish(&peer).unwrap();
    send(&mut ws, ClientMsg::Confirm(keys.confirmation(Role::Sender))).await;
    let tag = match recv(&mut ws).await {
        ServerMsg::Confirm(t) => t,
        other => panic!("expected Confirm, got {other:?}"),
    };
    if keys.verify(Role::Receiver, &tag).is_err() {
        send(
            &mut ws,
            ClientMsg::Fail(short_codes_spike::wire::FailReason::ConfirmMismatch),
        )
        .await;
    } else {
        send(
            &mut ws,
            ClientMsg::Sealed(keys.seal(ticket.as_bytes()).unwrap()),
        )
        .await;
    }
    let end = recv(&mut ws).await;
    (
        plate,
        if matches!(end, ServerMsg::Done) {
            Ok(())
        } else {
            Err(())
        },
    )
}

async fn receiver(
    addr: std::net::SocketAddr,
    tls_cert: Option<&rustls::pki_types::CertificateDer<'static>>,
    plate: u32,
    words: [&str; 2],
) -> Result<String, ()> {
    let mut ws = connect(addr, tls_cert).await;
    send(&mut ws, ClientMsg::Join(plate)).await;
    assert!(matches!(
        recv(&mut ws).await,
        ServerMsg::Joined(Role::Receiver)
    ));
    let code = Code::parse(&format!("{plate}-{}-{}", words[0], words[1])).unwrap();
    let started = Started::new(Role::Receiver, &code);
    send(&mut ws, ClientMsg::Spake(started.outbound.clone())).await;
    let peer = match recv(&mut ws).await {
        ServerMsg::Spake(m) => m,
        other => panic!("expected Spake, got {other:?}"),
    };
    let keys = started.finish(&peer).unwrap();
    send(
        &mut ws,
        ClientMsg::Confirm(keys.confirmation(Role::Receiver)),
    )
    .await;
    let tag = match recv(&mut ws).await {
        ServerMsg::Confirm(t) => t,
        other => panic!("expected Confirm, got {other:?}"),
    };
    if keys.verify(Role::Sender, &tag).is_err() {
        send(
            &mut ws,
            ClientMsg::Fail(short_codes_spike::wire::FailReason::ConfirmMismatch),
        )
        .await;
        return Err(());
    }
    let sealed = match recv(&mut ws).await {
        ServerMsg::Sealed(s) => s,
        other => panic!("expected Sealed, got {other:?}"),
    };
    let plain = keys.open(&sealed).map_err(|_| ())?;
    let ticket = String::from_utf8(plain).map_err(|_| ())?;
    send(&mut ws, ClientMsg::Done).await;
    Ok(ticket)
}

#[tokio::test]
async fn right_words_deliver_the_ticket() {
    let place = MeetingPlace::new(Config::default());
    let (addr, cert) = start(place, false).await;
    let ticket = "sluice1abcdefghjkmnpqrstvwxyz234567";
    let (tx, rx) = tokio::sync::oneshot::channel();
    let sender_task = tokio::spawn(sender(addr, cert, ticket, tx));
    let plate = rx.await.unwrap();
    let receiver_task =
        tokio::spawn(async move { receiver(addr, None, plate, ["orbit", "velvet"]).await });
    let (_, result) = sender_task.await.unwrap();
    assert!(result.is_ok(), "the sender saw Done");
    let got = receiver_task.await.unwrap().unwrap();
    assert_eq!(got, ticket, "the receiver opened the ticket");
}

#[tokio::test]
async fn wrong_words_burn_the_nameplate_and_the_sender_sees_it() {
    let place = MeetingPlace::new(Config::default());
    let (addr, cert) = start(place, false).await;
    let mut ws = connect(addr, cert.as_ref()).await;
    send(&mut ws, ClientMsg::Claim).await;
    let plate = match recv(&mut ws).await {
        ServerMsg::Claimed(p) => p,
        other => panic!("expected Claimed, got {other:?}"),
    };
    let good = Code::parse(&format!("{plate}-orbit-velvet")).unwrap();
    let bad = Code::parse(&format!("{plate}-orbit-vendor")).unwrap();

    let mut sender_ws = ws;
    let sender = Started::new(Role::Sender, &good);

    let mut bad_ws = connect(addr, None).await;
    send(&mut bad_ws, ClientMsg::Join(plate)).await;
    assert!(matches!(
        recv(&mut bad_ws).await,
        ServerMsg::Joined(Role::Receiver)
    ));
    assert!(matches!(
        recv(&mut sender_ws).await,
        ServerMsg::Joined(Role::Sender)
    ));
    let bad_started = Started::new(Role::Receiver, &bad);

    // The SPAKE2 messages cross through the meeting place.
    send(&mut sender_ws, ClientMsg::Spake(sender.outbound.clone())).await;
    send(&mut bad_ws, ClientMsg::Spake(bad_started.outbound.clone())).await;
    let peer_a = match recv(&mut sender_ws).await {
        ServerMsg::Spake(m) => m,
        other => panic!("expected Spake, got {other:?}"),
    };
    let peer_b = match recv(&mut bad_ws).await {
        ServerMsg::Spake(m) => m,
        other => panic!("expected Spake, got {other:?}"),
    };
    let keys_a = sender.finish(&peer_a).unwrap();
    let keys_b = bad_started.finish(&peer_b).unwrap();
    send(
        &mut sender_ws,
        ClientMsg::Confirm(keys_a.confirmation(Role::Sender)),
    )
    .await;
    send(
        &mut bad_ws,
        ClientMsg::Confirm(keys_b.confirmation(Role::Receiver)),
    )
    .await;

    let tag_b = match recv(&mut sender_ws).await {
        ServerMsg::Confirm(t) => t,
        other => panic!("expected Confirm, got {other:?}"),
    };
    assert!(keys_a.verify(Role::Receiver, &tag_b).is_err());
    send(
        &mut sender_ws,
        ClientMsg::Fail(short_codes_spike::wire::FailReason::ConfirmMismatch),
    )
    .await;
    match recv(&mut sender_ws).await {
        ServerMsg::Burned => {}
        other => panic!("the sender expected Burned, got {other:?}"),
    }
    match recv(&mut bad_ws).await {
        ServerMsg::Confirm(_) => {
            // The sender's own tag is wrong for the receiver too; the burn follows.
            match recv(&mut bad_ws).await {
                ServerMsg::Burned => {}
                other => panic!("the receiver expected Burned, got {other:?}"),
            }
        }
        ServerMsg::Burned => {}
        other => panic!("the receiver expected Burned or its tag, got {other:?}"),
    }

    // The plate is gone, so a third connection finds no such code.
    let mut third = connect(addr, None).await;
    send(&mut third, ClientMsg::Join(plate)).await;
    match recv(&mut third).await {
        ServerMsg::Error(ErrorCode::NoCode) => {}
        other => panic!("a burned plate answered {other:?}"),
    }
}

#[tokio::test]
async fn a_third_connection_to_a_held_nameplate_is_refused() {
    let place = MeetingPlace::new(Config::default());
    let (addr, cert) = start(place, false).await;
    let mut first = connect(addr, cert.as_ref()).await;
    send(&mut first, ClientMsg::Claim).await;
    let plate = match recv(&mut first).await {
        ServerMsg::Claimed(p) => p,
        other => panic!("expected Claimed, got {other:?}"),
    };
    let mut second = connect(addr, None).await;
    send(&mut second, ClientMsg::Join(plate)).await;
    assert!(matches!(
        recv(&mut second).await,
        ServerMsg::Joined(Role::Receiver)
    ));
    let mut third = connect(addr, None).await;
    send(&mut third, ClientMsg::Join(plate)).await;
    match recv(&mut third).await {
        ServerMsg::Error(ErrorCode::Busy) => {}
        other => panic!("a third connection got {other:?}"),
    }
}

#[tokio::test]
async fn the_relay_and_the_meeting_place_share_one_tls_port() {
    let place = MeetingPlace::new(Config::default());
    let (addr, cert) = start(place, true).await;
    let cert = cert.unwrap();

    // The relay still relays: two clients handshake, ping, and send a datagram to each other.
    let key_a = SecretKey::generate();
    let key_b = SecretKey::generate();
    let a_id = key_a.public();
    let b_id = key_b.public();
    let url = Url::parse(&format!("https://127.0.0.1:{}", addr.port())).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let tls = CaTlsConfig::custom_roots([cert.clone()])
        .client_config(provider)
        .unwrap();
    let mut a = ClientBuilder::new(url.clone(), key_a, DnsResolver::new())
        .tls_client_config(tls.clone())
        .connect()
        .await
        .expect("relay client a");
    let mut b = ClientBuilder::new(url, key_b, DnsResolver::new())
        .tls_client_config(tls)
        .connect()
        .await
        .expect("relay client b");
    a.send(ClientToRelayMsg::Ping([1; 8])).await.unwrap();
    match a.next().await.unwrap().unwrap() {
        RelayToClientMsg::Pong { .. } => {}
        other => panic!("expected Pong, got {other:?}"),
    }
    a.send(ClientToRelayMsg::Datagrams {
        dst_endpoint_id: b_id,
        datagrams: Datagrams::from(b"hello through the relay"),
    })
    .await
    .unwrap();
    let received = b.next().await.unwrap().unwrap();
    match received {
        RelayToClientMsg::Datagrams {
            remote_endpoint_id,
            datagrams,
        } => {
            assert_eq!(remote_endpoint_id, a_id);
            assert_eq!(datagrams, Datagrams::from(b"hello through the relay"));
        }
        other => panic!("expected Datagrams, got {other:?}"),
    }

    // And the meeting place answers a wss client on the same port and certificate.
    let mut meet = connect(addr, Some(&cert)).await;
    send(&mut meet, ClientMsg::Claim).await;
    match recv(&mut meet).await {
        ServerMsg::Claimed(_) => {}
        other => panic!("expected Claimed over wss, got {other:?}"),
    }

    let _ = tokio::time::timeout(Duration::from_secs(1), a.close()).await;
}
