//! One TLS listener that serves `iroh-relay` at `/relay` and the meeting place at `/meet`, the
//! plain-HTTP captive-portal check, QUIC address discovery on its own port, and metrics when the
//! config asks for them.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{HeaderMap, Method, Request, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use iroh_relay::KeyCache;
use iroh_relay::server::http_server::{Handlers, RelayService, TlsConfig};
use iroh_relay::server::{ClientRateLimit, Metrics, QuicConfig, Server, ServerConfig};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::meet::MeetHttp;

/// How long a connection has to finish TLS and the WebSocket handshake.
const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(30);

/// A running relay. Dropping it aborts the listener tasks; [`Running::shutdown`] also stops QUIC.
pub struct Running {
    /// The address the relay service answers on, which a test connects to.
    pub addr: SocketAddr,
    listener: JoinHandle<()>,
    captive: Option<JoinHandle<()>>,
    server: Option<Server>,
}

impl Running {
    /// Stops every listener and the QUIC server.
    pub async fn shutdown(mut self) -> Result<()> {
        self.listener.abort();
        if let Some(captive) = self.captive.take() {
            captive.abort();
        }
        if let Some(server) = self.server.take() {
            server
                .shutdown()
                .await
                .context("cannot stop the QUIC server")?;
        }
        Ok(())
    }
}

/// Starts the relay from `config`. The listeners bind their configured addresses; a test passes
/// port 0 and reads [`Running::addr`].
pub async fn start(config: Config, meet: Option<Arc<MeetHttp>>) -> Result<Running> {
    let tls = config
        .tls
        .as_ref()
        .map(load_tls)
        .transpose()
        .context("cannot read the TLS certificate")?;
    let mut handlers = Handlers::default();
    if let Some(meet) = &meet {
        handlers.insert((Method::GET, "/meet"), Box::new(meet.handler()));
    }
    let access = crate::access::build(
        &config.access,
        std::env::var("IROH_RELAY_HTTP_BEARER_TOKEN").ok(),
    )?;
    let rate = config.limits.client_rx.map(|rate| {
        let mut limit = ClientRateLimit::new(rate);
        limit.max_burst_bytes = config.limits.max_burst_bytes;
        limit
    });
    let service = RelayService::new(
        handlers,
        HeaderMap::new(),
        rate,
        KeyCache::new(config.key_cache_capacity.unwrap_or(1024)),
        access,
        Arc::new(Metrics::default()),
    );

    let relay_bind = config.relay_bind_addr();
    let listener = TcpListener::bind(relay_bind)
        .await
        .with_context(|| format!("cannot listen on {relay_bind}"))?;
    let addr = listener
        .local_addr()
        .context("cannot read the relay address")?;
    let tls_for_conn = tls.as_ref().map(|tls| TlsConfig::new(tls.clone()));
    let accept_service = service.clone();
    let listener = tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let service = accept_service.clone();
            let tls = tls_for_conn.clone();
            tokio::spawn(crate::meet::PEER_IP.scope(
                peer.ip(),
                service.handle_connection(stream, tls, ESTABLISH_TIMEOUT),
            ));
        }
    });

    let captive = match (&tls, config.captive_bind_addr() != addr) {
        (Some(_), true) => {
            let address = config.captive_bind_addr();
            let captive_listener = TcpListener::bind(address)
                .await
                .with_context(|| format!("cannot listen on {address}"))?;
            Some(tokio::spawn(async move {
                loop {
                    let Ok((stream, _)) = captive_listener.accept().await else {
                        break;
                    };
                    tokio::spawn(async move {
                        let io = TokioIo::new(stream);
                        let service = service_fn(|request: Request<Incoming>| async move {
                            let status = if request.uri().path() == "/generate_204" {
                                StatusCode::NO_CONTENT
                            } else {
                                StatusCode::NOT_FOUND
                            };
                            Ok::<_, std::convert::Infallible>(
                                Response::builder()
                                    .status(status)
                                    .body(Full::new(Bytes::new()))
                                    .expect("a fixed response"),
                            )
                        });
                        let _ = hyper::server::conn::http1::Builder::new()
                            .serve_connection(io, service)
                            .await;
                    });
                }
            }))
        }
        _ => None,
    };

    let mut quic_server = None;
    if let (Some(tls), Some(quic_addr)) =
        (&tls, config.tls.as_ref().and_then(|tls| tls.quic_bind_addr))
    {
        let mut server_config = ServerConfig::default();
        server_config.relay = None;
        let mut quic_config = QuicConfig::new(quic_addr);
        quic_config.server_config = Some((**tls).clone());
        server_config.quic = Some(quic_config);
        server_config.metrics_addr = config.metrics_addr;
        let server = Server::spawn(server_config)
            .await
            .context("cannot start the QUIC server")?;
        quic_server = Some(server);
    }

    Ok(Running {
        addr,
        listener,
        captive,
        server: quic_server,
    })
}

fn load_tls(config: &crate::config::Tls) -> Result<Arc<rustls::ServerConfig>> {
    let cert_path = config
        .manual_cert_path
        .clone()
        .unwrap_or_else(|| config.cert_dir.join("default.crt"));
    let key_path = config
        .manual_key_path
        .clone()
        .unwrap_or_else(|| config.cert_dir.join("default.key"));
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(&cert_path)
        .with_context(|| format!("cannot read {}", cert_path.display()))?
        .collect::<Result<_, _>>()
        .with_context(|| format!("cannot read {}", cert_path.display()))?;
    let key: PrivateKeyDer<'static> = PrivateKeyDer::from_pem_file(&key_path)
        .with_context(|| format!("cannot read {}", key_path.display()))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .context("the TLS provider has no safe protocol versions")?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .context("the certificate and key do not make a server config")?;
    Ok(Arc::new(server))
}
