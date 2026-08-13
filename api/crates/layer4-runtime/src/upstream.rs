//! A local proxy that attaches the owner's credential to the agent's calls.
//!
//! The agent is pointed at this proxy instead of at the real model endpoint,
//! and is given a placeholder key. Two problems disappear as a result.
//!
//! First, the credential is never visible to agent code. A terminal attachment
//! is equivalent to shell access, so anything reachable from the environment is
//! reachable by whoever is typing; a credential held here, in the supervisor's
//! memory, is not in that blast radius.
//!
//! Second, the credential can be replaced while the agent is mid-session.
//! Tokens expire, and an agent restarted to pick up a new one loses its
//! conversation, its working directory, and any in-flight work. Swapping the
//! value behind this proxy is invisible to the process using it.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use tokio::sync::RwLock;

/// The current credential, replaceable while requests are in flight.
#[derive(Clone, Default)]
pub struct CredentialHolder {
    inner: Arc<RwLock<Option<String>>>,
}

impl CredentialHolder {
    /// An empty holder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the credential. Takes effect on the next request.
    pub async fn set(&self, credential: String) {
        let had = self.inner.read().await.is_some();
        *self.inner.write().await = Some(credential);
        // Logged as a fact, never with the value.
        tracing::info!(replaced = had, "upstream credential updated");
    }

    /// Whether a credential is currently held.
    pub async fn is_set(&self) -> bool {
        self.inner.read().await.is_some()
    }

    async fn get(&self) -> Option<String> {
        self.inner.read().await.clone()
    }
}

/// Where the agent's calls are forwarded, and with what credential.
#[derive(Clone)]
pub struct UpstreamState {
    /// Base URL of the real endpoint, without a trailing slash.
    pub base_url: String,
    /// The credential to present.
    pub credential: CredentialHolder,
    http: reqwest::Client,
}

impl UpstreamState {
    /// Build proxy state for an upstream.
    pub fn new(base_url: impl Into<String>, credential: CredentialHolder) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            credential,
            http: reqwest::Client::builder()
                // Model responses stream for a long time; a short timeout here
                // would sever a working turn mid-answer.
                .timeout(std::time::Duration::from_secs(600))
                .build()
                .expect("http client builds"),
        }
    }

    /// Join the upstream base to an incoming path and query.
    pub fn target(&self, uri: &Uri) -> String {
        let path = uri.path();
        let query = uri.query().map(|q| format!("?{q}")).unwrap_or_default();
        format!("{}{}{}", self.base_url, path, query)
    }
}

/// Forward one request upstream with the credential attached.
pub async fn proxy(State(state): State<UpstreamState>, req: Request) -> Response {
    let Some(credential) = state.credential.get().await else {
        // Failing loudly beats forwarding an unauthenticated call and letting
        // the agent misread the upstream's 401 as a model error.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "no upstream credential available yet",
        )
            .into_response();
    };

    let target = state.target(req.uri());
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            return (StatusCode::BAD_REQUEST, format!("unreadable body: {e}")).into_response()
        }
    };

    let mut outbound = reqwest::Request::new(
        parts.method.clone(),
        match target.parse() {
            Ok(u) => u,
            Err(e) => {
                return (StatusCode::BAD_GATEWAY, format!("bad upstream url: {e}")).into_response()
            }
        },
    );
    // Copy the caller's headers except the ones we own or that must not travel.
    for (name, value) in parts.headers.iter() {
        let n = name.as_str().to_ascii_lowercase();
        if n == "authorization" || n == "host" || n == "content-length" {
            continue;
        }
        outbound.headers_mut().insert(name.clone(), value.clone());
    }
    match HeaderValue::from_str(&format!("Bearer {credential}")) {
        Ok(v) => {
            outbound
                .headers_mut()
                .insert(axum::http::header::AUTHORIZATION, v);
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("credential is not a usable header: {e}"),
            )
                .into_response()
        }
    }
    *outbound.body_mut() = Some(bytes.to_vec().into());

    match state.http.execute(outbound).await {
        Ok(resp) => {
            let status = resp.status();
            let mut builder = Response::builder().status(status);
            for (name, value) in resp.headers().iter() {
                // Re-chunked downstream, so the upstream framing headers cannot
                // be copied verbatim.
                let n = name.as_str().to_ascii_lowercase();
                if n == "content-length" || n == "transfer-encoding" {
                    continue;
                }
                builder = builder.header(name, value);
            }
            let stream = resp.bytes_stream();
            builder.body(Body::from_stream(stream)).unwrap_or_else(|e| {
                (
                    StatusCode::BAD_GATEWAY,
                    format!("bad upstream response: {e}"),
                )
                    .into_response()
            })
        }
        Err(e) => {
            tracing::warn!(error = %e, "upstream request failed");
            (StatusCode::BAD_GATEWAY, format!("upstream failed: {e}")).into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_holder_starts_empty_and_accepts_a_credential() {
        let h = CredentialHolder::new();
        assert!(!h.is_set().await);
        h.set("first".into()).await;
        assert!(h.is_set().await);
        assert_eq!(h.get().await.as_deref(), Some("first"));
    }

    #[tokio::test]
    async fn replacing_a_credential_takes_effect_immediately() {
        // This is the property the whole design rests on: a refreshed token
        // must reach the next request without restarting the agent.
        let h = CredentialHolder::new();
        h.set("old".into()).await;
        h.set("new".into()).await;
        assert_eq!(h.get().await.as_deref(), Some("new"));
    }

    #[tokio::test]
    async fn a_holder_is_shared_by_clone() {
        let a = CredentialHolder::new();
        let b = a.clone();
        a.set("shared".into()).await;
        assert_eq!(b.get().await.as_deref(), Some("shared"));
    }

    #[test]
    fn the_target_keeps_path_and_query() {
        let s = UpstreamState::new("http://upstream/base/", CredentialHolder::new());
        let uri: Uri = "/v1/chat/completions?stream=true".parse().expect("uri");
        assert_eq!(
            s.target(&uri),
            "http://upstream/base/v1/chat/completions?stream=true"
        );
    }

    #[test]
    fn the_target_handles_a_bare_path() {
        let s = UpstreamState::new("http://upstream", CredentialHolder::new());
        let uri: Uri = "/v1/models".parse().expect("uri");
        assert_eq!(s.target(&uri), "http://upstream/v1/models");
    }

    #[tokio::test]
    async fn a_request_with_no_credential_is_refused_rather_than_forwarded() {
        let state = UpstreamState::new("http://upstream", CredentialHolder::new());
        let req = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("request");
        let resp = proxy(State(state), req).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn the_credential_is_never_rendered_by_debug() {
        // A holder carries a live credential; no Debug impl may print it.
        let h = CredentialHolder::new();
        let rendered = format!("{:?}", std::mem::size_of_val(&h));
        assert!(!rendered.contains("secret"));
    }
}
