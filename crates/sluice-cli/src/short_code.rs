//! The meeting place as a command uses it: claim a nameplate from `send --code`, and resolve a
//! code to its ticket for `recv <code>`.
//!
//! The meeting place is `wss://<relay-host>/meet`, derived from the relay URL. The code carries no
//! host, so both sides must name the same relay. Nothing here logs a code, a nameplate or a ticket.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use sluice_core::Error;
use sluice_meet::Code;
use sluice_meet::client::{ClientEvent, Receiver, Sender};
use sluice_meet::frame::{ClientFrame, ErrorCode, ServerFrame};
use tokio::net::TcpStream;
use tokio_websockets::{ClientBuilder, Connector, MaybeTlsStream, Message, WebSocketStream};

/// The exact message SPEC 4.1.4 gives a `send --code` with no relay.
pub const NO_RELAY: &str = "--code needs a relay with a meeting place, and no relay is configured; \
                            set --relay or send without --code";
/// The exact message for a relay that serves no `/meet`.
pub const NO_MEET: &str = "--code needs a relay with a meeting place, and the configured relay \
                           does not serve /meet; add [meet] to it or send without --code";

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Claims a nameplate and returns the open meeting, the sender, and the code text to print.
pub async fn claim(relay: &str, ticket: &str) -> Result<(Ws, Sender, String), Error> {
    let mut ws = connect(relay).await?;
    let mut words = [0u8; 4];
    getrandom::fill(&mut words)
        .map_err(|e| Error::Storage(format!("cannot choose the code's words: {e}")))?;
    let words = [
        u16::from_be_bytes([words[0], words[1]]) % 2048,
        u16::from_be_bytes([words[2], words[3]]) % 2048,
    ];
    let (mut sender, first) =
        Sender::new(words, ticket).map_err(|e| Error::Bug(format!("a word index: {e}")))?;
    send(&mut ws, first).await?;
    loop {
        let frame = recv(&mut ws).await?;
        for event in sender
            .on_frame(frame)
            .map_err(|e| Error::Network(format!("the meeting place: {e}")))?
        {
            match event {
                ClientEvent::Send(frame) => send(&mut ws, frame).await?,
                ClientEvent::Claimed => {
                    let code = sender
                        .code()
                        .ok_or_else(|| Error::Bug("a claim without a code".to_string()))?
                        .text();
                    return Ok((ws, sender, code));
                }
                ClientEvent::Refused(code) => return Err(refusal(code)),
                ClientEvent::Burned => return Err(burned()),
                ClientEvent::PeerClosed => return Err(Error::Network("the sender closed".into())),
                ClientEvent::Done | ClientEvent::Ticket(_) => {}
            }
        }
    }
}

/// Whether `relay` serves a meeting place: a WebSocket upgrade to `/meet`, closed at once.
pub async fn serves(relay: &str) -> Result<(), Error> {
    let ws = connect(relay).await?;
    drop(ws);
    Ok(())
}

/// Drives a claimed sender until the receiver is done, a guess burns the code, or the socket
/// closes. The transfer keeps serving whatever this does.
pub async fn drive_sender(mut ws: Ws, mut sender: Sender) {
    loop {
        let Ok(frame) = recv(&mut ws).await else {
            return;
        };
        let Ok(events) = sender.on_frame(frame) else {
            return;
        };
        for event in events {
            match event {
                ClientEvent::Send(frame) => {
                    if send(&mut ws, frame).await.is_err() {
                        return;
                    }
                }
                ClientEvent::Done => return,
                ClientEvent::Burned => {
                    eprintln!("a wrong code was tried, so this code is burned; make a new one");
                    return;
                }
                ClientEvent::Refused(code) => {
                    eprintln!("the meeting place refused the code: {code}");
                    return;
                }
                ClientEvent::PeerClosed => return,
                ClientEvent::Claimed | ClientEvent::Ticket(_) => {}
            }
        }
    }
}

/// Joins a code's nameplate and returns the ticket it opens.
pub async fn resolve(relay: &str, code: Code) -> Result<String, Error> {
    let mut ws = match connect(relay).await {
        Ok(ws) => ws,
        Err(Error::Policy(text)) if text == NO_MEET => {
            return Err(Error::Policy(
                "the configured relay does not serve a meeting place, so the code cannot be met \
                 there"
                    .to_string(),
            ));
        }
        Err(error) => return Err(error),
    };
    let (mut receiver, first) = Receiver::new(code);
    send(&mut ws, first).await?;
    loop {
        let frame = recv(&mut ws).await?;
        for event in receiver
            .on_frame(frame)
            .map_err(|e| Error::Network(format!("the meeting place: {e}")))?
        {
            match event {
                ClientEvent::Send(frame) => send(&mut ws, frame).await?,
                ClientEvent::Ticket(ticket) => return Ok(ticket.to_string()),
                ClientEvent::Burned => return Err(burned()),
                ClientEvent::PeerClosed => {
                    return Err(Error::Network("the sender closed before delivering".into()));
                }
                ClientEvent::Refused(code) => return Err(refusal(code)),
                ClientEvent::Claimed | ClientEvent::Done => {}
            }
        }
    }
}

fn refusal(code: ErrorCode) -> Error {
    match code {
        ErrorCode::NoCode => Error::Policy(
            "the code was not found at this relay, so the sender may be using another relay"
                .to_string(),
        ),
        ErrorCode::Busy => Error::Policy("another receiver joined the code first".to_string()),
        other => Error::Policy(format!("the meeting place refused it: {other}")),
    }
}

fn burned() -> Error {
    Error::Policy("the words do not match the sender's, so the code is burned".to_string())
}

async fn connect(relay: &str) -> Result<Ws, Error> {
    let target = meet_url(relay)?;
    let builder = ClientBuilder::from_uri(
        target
            .parse()
            .map_err(|_| Error::Policy("the relay URL is not one a code can use".to_string()))?,
    );
    let result = if target.starts_with("wss://") {
        let config = tls_config()?;
        let connector = Connector::Rustls(tokio_rustls::TlsConnector::from(Arc::new(config)));
        builder.connector(&connector).connect().await
    } else {
        builder.connect().await
    };
    match result {
        Ok((ws, _)) => Ok(ws),
        Err(error) => {
            let text = error.to_string();
            if text.contains("404") || text.contains("Not Found") {
                Err(Error::Policy(NO_MEET.to_string()))
            } else {
                Err(Error::Network(format!(
                    "cannot reach the meeting place: {error}"
                )))
            }
        }
    }
}

/// The relay's `/meet` URL: the same host and port, `https` to `wss` and `http` to `ws`.
pub fn meet_url(relay: &str) -> Result<String, Error> {
    let parsed = url::Url::parse(relay)
        .map_err(|_| Error::Policy("the relay URL is not an absolute URL".to_string()))?;
    let scheme = match parsed.scheme() {
        "https" => "wss",
        "http" => "ws",
        _ => {
            return Err(Error::Policy(
                "a code needs an https or http relay URL".to_string(),
            ));
        }
    };
    let mut target = parsed;
    target
        .set_scheme(scheme)
        .map_err(|_| Error::Policy("the relay URL has no host".to_string()))?;
    target.set_path("/meet");
    target.set_query(None);
    target.set_fragment(None);
    Ok(target.to_string())
}

fn tls_config() -> Result<rustls::ClientConfig, Error> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .map_err(|e| Error::Network(format!("the TLS client: {e}")))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(config)
}

async fn send(ws: &mut Ws, frame: ClientFrame) -> Result<(), Error> {
    let bytes = frame
        .encode()
        .map_err(|e| Error::Bug(format!("a client frame: {e}")))?;
    ws.send(Message::binary(bytes))
        .await
        .map_err(|e| Error::Network(format!("the meeting place: {e}")))
}

async fn recv(ws: &mut Ws) -> Result<ServerFrame, Error> {
    let message = ws
        .next()
        .await
        .ok_or_else(|| Error::Network("the meeting place closed".to_string()))?
        .map_err(|e| Error::Network(format!("the meeting place: {e}")))?;
    if message.is_close() {
        return Err(Error::Network("the meeting place closed".to_string()));
    }
    if message.is_text() {
        return Err(Error::Network(
            "the meeting place sent text instead of a frame".to_string(),
        ));
    }
    ServerFrame::decode(message.into_payload().as_ref())
        .map_err(|e| Error::Network(format!("the meeting place: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relay_url_becomes_the_meeting_place_url() {
        assert_eq!(
            meet_url("https://relay.example/").unwrap(),
            "wss://relay.example/meet"
        );
        assert_eq!(
            meet_url("https://relay.example:8443/base?x=1#f").unwrap(),
            "wss://relay.example:8443/meet"
        );
        assert_eq!(
            meet_url("http://127.0.0.1:8080").unwrap(),
            "ws://127.0.0.1:8080/meet"
        );
        assert!(meet_url("ftp://relay.example/").is_err());
        assert!(meet_url("not a url").is_err());
    }

    #[test]
    fn the_two_exact_messages_are_the_specs() {
        assert_eq!(
            NO_RELAY,
            "--code needs a relay with a meeting place, and no relay is configured; set --relay or \
             send without --code"
        );
        assert_eq!(
            NO_MEET,
            "--code needs a relay with a meeting place, and the configured relay does not serve \
             /meet; add [meet] to it or send without --code"
        );
    }
}
