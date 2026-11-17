//! One TLS listener that serves `iroh-relay` at `/relay` and the meeting place at `/meet`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use http::header::{CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_KEY, UPGRADE};
use http::response::Builder as ResponseBuilder;
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use iroh_relay::KeyCache;
use iroh_relay::server::http_server::{BytesBody, Handlers, HyperResult, RelayService, TlsConfig};
use iroh_relay::server::{AllowAll, Metrics};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::net::TcpListener;

use tokio_websockets::ServerBuilder;

use crate::meet::{MeetingPlace, serve_meet};

tokio::task_local! {
    /// The peer address of the connection whose request is being handled. `RelayService` does not
    /// pass it to a `Handlers` closure, so the accept loop scopes it around the connection.
    static PEER_IP: IpAddr;
}

/// A self-signed certificate for the loopback host, and the relay's TLS config using it.
pub fn self_signed() -> anyhow::Result<(TlsConfig, CertificateDer<'static>)> {
    let (config, cert) = self_signed_config()?;
    Ok((TlsConfig::new(config), cert))
}

/// The raw server config, for the QUIC-only server and for callers that build their own wrapper.
pub fn self_signed_config() -> anyhow::Result<(Arc<rustls::ServerConfig>, CertificateDer<'static>)> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])?;
    params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    let cert = params.self_signed(&key)?;
    let cert_der = cert.der().clone();
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der()));
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(vec![cert_der.clone()], key_der)?;
    Ok((Arc::new(config), cert_der))
}

/// A client configuration that trusts exactly the spike's certificate.
pub fn client_config(cert: &CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.clone()).expect("the spike's certificate");
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("ring's protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    Arc::new(config)
}

/// Starts the server. Returns the local address it bound.
pub async fn spawn(
    addr: SocketAddr,
    place: Arc<MeetingPlace>,
    tls: Option<TlsConfig>,
) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(addr).await?;
    let local = listener.local_addr()?;

    let mut handlers = Handlers::default();
    handlers.insert((Method::GET, "/meet"), Box::new(meet_handler(place)));
    let relay = RelayService::new(
        handlers,
        HeaderMap::new(),
        None,
        KeyCache::new(1024),
        Arc::new(AllowAll),
        Arc::new(Metrics::default()),
    );

    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let service = relay.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                PEER_IP
                    .scope(
                        peer.ip(),
                        service.handle_connection(stream, tls, Duration::from_secs(30)),
                    )
                    .await;
            });
        }
    });
    Ok(local)
}

fn meet_handler(
    place: Arc<MeetingPlace>,
) -> impl Fn(Request<Incoming>, ResponseBuilder) -> HyperResult<Response<BytesBody>> + Send + Sync + 'static
{
    move |req: Request<Incoming>, _builder: ResponseBuilder| {
        let Some(key) = req.headers().get(SEC_WEBSOCKET_KEY).cloned() else {
            return Ok(response(StatusCode::BAD_REQUEST, "expected a WebSocket"));
        };
        let accept = accept_key(key.as_bytes());
        let ip = PEER_IP
            .try_with(|ip| *ip)
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let place = place.clone();
        tokio::spawn(async move {
            if let Ok(upgraded) = hyper::upgrade::on(req).await {
                let ws = ServerBuilder::new().serve(hyper_util::rt::TokioIo::new(upgraded));
                serve_meet(place, ws, ip).await;
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
}

fn empty() -> BytesBody {
    Box::new(Full::new(Bytes::new()))
}

fn response(status: StatusCode, text: &'static str) -> Response<BytesBody> {
    Response::builder()
        .status(status)
        .body({
            let body: BytesBody = Box::new(Full::new(Bytes::from_static(text.as_bytes())));
            body
        })
        .expect("a fixed response")
}

fn accept_key(key: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut sha1 = Sha1::new();
    sha1.update(key);
    sha1.update(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(sha1.finalize())
}


#[cfg(test)]
mod tests {
    use super::*;

    /// The design keeps QUIC address discovery on its own port. That path is the library's
    /// `Server` with no relay half, using the same TLS config the HTTPS listener uses.
    #[tokio::test]
    async fn quic_address_discovery_runs_without_the_http_relay() {
        use iroh_relay::server::{QuicConfig, Server as RelayServer, ServerConfig};

        let (config, _cert) = self_signed_config().unwrap();
        let mut server_config = ServerConfig::default();
        server_config.relay = None;
        let mut quic = QuicConfig::new("127.0.0.1:0".parse::<SocketAddr>().unwrap());
        quic.server_config = Some((*config).clone());
        server_config.quic = Some(quic);
        let server = RelayServer::spawn(server_config).await.expect("QUIC-only server");
        server.shutdown().await.expect("graceful shutdown");
    }
}
