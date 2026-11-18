//! The access rules `iroh-relay` 1.4.1 has today, with the meeting place sharing the same policy:
//! an allowlist, a denylist, shared tokens, an HTTP check, or everyone. The config's strings are
//! parsed here, so a bad one is refused before a listener starts.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use iroh_base::EndpointId;
use iroh_relay::server::{Access, AccessControl, ClientRequest, DynAccessControl};
use reqwest::StatusCode;

use crate::config::AccessConfig;

/// The headers iroh-relay 1.4.1 sends to an HTTP access check.
const X_IROH_ENDPOINT_ID: &str = "X-Iroh-Endpoint-Id";
const X_IROH_NODE_ID_DEPRECATED: &str = "X-Iroh-NodeId";

/// Builds the access control a config names.
pub fn build(
    config: &AccessConfig,
    bearer_token: Option<String>,
) -> Result<Arc<dyn DynAccessControl>> {
    Ok(match config {
        AccessConfig::Everyone => Arc::new(iroh_relay::server::AllowAll),
        AccessConfig::Allowlist(ids) => Arc::new(Allowlist(parse_ids(ids)?)),
        AccessConfig::Denylist(ids) => Arc::new(Denylist(parse_ids(ids)?)),
        AccessConfig::SharedToken(tokens) => Arc::new(SharedTokens(tokens.clone())),
        AccessConfig::Http(http) => {
            let token = bearer_token.or_else(|| http.bearer_token.clone());
            Arc::new(HttpCheck {
                client: reqwest::Client::builder()
                    .build()
                    .context("cannot build the HTTP access client")?,
                url: http.url.clone(),
                token,
            })
        }
    })
}

fn parse_ids(ids: &[String]) -> Result<Vec<EndpointId>> {
    ids.iter()
        .map(|id| {
            EndpointId::from_str(id)
                .with_context(|| format!("an access id is not an endpoint id: {id}"))
        })
        .collect()
}

#[derive(Debug)]
struct Allowlist(Vec<EndpointId>);

impl AccessControl for Allowlist {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(&request.endpoint_id()) {
            Access::Allow
        } else {
            Access::Deny { reason: None }
        }
    }
}

#[derive(Debug)]
struct Denylist(Vec<EndpointId>);

impl AccessControl for Denylist {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        if self.0.contains(&request.endpoint_id()) {
            Access::Deny { reason: None }
        } else {
            Access::Allow
        }
    }
}

#[derive(Debug)]
struct SharedTokens(Vec<String>);

impl AccessControl for SharedTokens {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        match request.auth_token() {
            Some(token) if self.0.contains(&token) => Access::Allow,
            _ => Access::Deny { reason: None },
        }
    }
}

#[derive(Debug)]
struct HttpCheck {
    client: reqwest::Client,
    url: String,
    token: Option<String>,
}

impl AccessControl for HttpCheck {
    async fn on_connect(&self, request: &ClientRequest) -> Access {
        let mut post = self
            .client
            .post(&self.url)
            .header(X_IROH_ENDPOINT_ID, request.endpoint_id().to_string())
            .header(X_IROH_NODE_ID_DEPRECATED, request.endpoint_id().to_string());
        if let Some(token) = &self.token {
            post = post.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        match post.send().await {
            Ok(response) if response.status() == StatusCode::OK => match response.text().await {
                Ok(text) if text == "true" => Access::Allow,
                _ => Access::Deny { reason: None },
            },
            _ => Access::Deny { reason: None },
        }
    }
}
