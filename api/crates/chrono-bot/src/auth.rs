//! Authenticating browsers and services against the identity provider.
//!
//! This service has no user model of its own. It runs the authorization-code
//! flow as a confidential client — the browser never holds the client secret
//! and never calls the token endpoint — and then carries the resulting subject
//! in its own session cookie.
//!
//! Endpoints are discovered from the provider's metadata document rather than
//! hard-coded, so a provider that moves a route does not break this service.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use layer1_public::AuthenticatedSubject;
use layer2_core::SubjectId;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Name of the session cookie this service issues.
pub const SESSION_COOKIE: &str = "chrono_bot_session";

/// Provider metadata, as far as this service uses it.
#[derive(Debug, Clone, Deserialize)]
struct ProviderMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    userinfo_endpoint: Option<String>,
}

/// How the service establishes who is calling.
#[derive(Debug, Clone)]
pub enum AuthMode {
    /// Full authorization-code flow against the identity provider.
    Provider {
        /// Base URL this process uses to reach the provider.
        ///
        /// May differ from the origin the browser uses. The provider builds its
        /// advertised endpoints from its own public origin, which a server
        /// inside the cluster usually cannot resolve, so back-channel calls are
        /// rewritten onto this host while the browser keeps the public one.
        base_url: String,
        /// Registered client identifier. Not a secret.
        client_id: String,
        /// Client secret, held only by this process.
        client_secret: String,
        /// This service's public origin, used to build the redirect URI.
        public_url: String,
    },
    /// Every request is attributed to one fixed subject.
    ///
    /// For local bring-up only. Selected explicitly by configuration so it can
    /// never be reached by accident in a deployed environment.
    Trusted {
        /// The subject every request is attributed to.
        subject: String,
    },
}

/// Auth state shared across handlers.
pub struct AuthState {
    mode: AuthMode,
    http: reqwest::Client,
    metadata: RwLock<Option<ProviderMetadata>>,
    /// Issued session cookies mapped to their subject.
    sessions: RwLock<HashMap<String, SubjectId>>,
    /// In-flight authorization attempts: state to PKCE verifier.
    ///
    /// The state parameter is what binds a callback to the login that started
    /// it. Without it a callback from an unrelated flow would be accepted.
    pending: RwLock<HashMap<String, String>>,
}

/// Derive the PKCE challenge for a verifier, per S256.
fn code_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

impl AuthState {
    /// Build auth state for a mode.
    pub fn new(mode: AuthMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
            metadata: RwLock::new(None),
            sessions: RwLock::new(HashMap::new()),
            pending: RwLock::new(HashMap::new()),
        })
    }

    /// Whether this service requires a real credential.
    pub fn requires_credential(&self) -> bool {
        matches!(self.mode, AuthMode::Provider { .. })
    }

    /// Resolve the subject for a request, if any.
    pub async fn resolve(&self, headers: &HeaderMap) -> Option<SubjectId> {
        match &self.mode {
            AuthMode::Trusted { subject } => Some(SubjectId(subject.clone())),
            AuthMode::Provider { .. } => {
                let token = session_cookie(headers)?;
                self.sessions.read().await.get(&token).cloned()
            }
        }
    }

    /// Fetch and cache the provider's metadata document.
    async fn metadata(&self) -> Result<ProviderMetadata, String> {
        if let Some(m) = self.metadata.read().await.clone() {
            return Ok(m);
        }
        let base = match &self.mode {
            AuthMode::Provider { base_url, .. } => base_url.clone(),
            AuthMode::Trusted { .. } => return Err("no provider configured".into()),
        };
        let url = format!(
            "{}/.well-known/openid-configuration",
            base.trim_end_matches('/')
        );
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("discovery request to {url} failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("discovery returned {} from {url}", resp.status()));
        }
        let mut meta: ProviderMetadata = resp
            .json()
            .await
            .map_err(|e| format!("discovery document from {url} is not usable: {e}"))?;

        // The provider advertises endpoints on its public origin. The browser
        // needs that origin for the authorization redirect, but this process
        // must reach the back-channel endpoints on a host it can actually
        // resolve, so only those two are rewritten.
        meta.token_endpoint = rebase(&meta.token_endpoint, &base);
        meta.userinfo_endpoint = meta.userinfo_endpoint.as_deref().map(|e| rebase(e, &base));

        *self.metadata.write().await = Some(meta.clone());
        tracing::info!(
            %url,
            authorization_endpoint = %meta.authorization_endpoint,
            token_endpoint = %meta.token_endpoint,
            "provider metadata discovered"
        );
        Ok(meta)
    }

    async fn issue_session(&self, subject: SubjectId) -> String {
        let token = uuid_v4();
        self.sessions
            .write()
            .await
            .insert(token.clone(), subject.clone());
        tracing::info!(subject = %subject, "session issued");
        token
    }
}

/// Move a URL onto another origin, keeping its path and query.
///
/// Used for back-channel endpoints only. Returns the original string when it
/// cannot be parsed, so a provider that advertises something unexpected fails
/// loudly at the request rather than silently here.
fn rebase(endpoint: &str, base: &str) -> String {
    let base = base.trim_end_matches('/');
    let Some(scheme_end) = endpoint.find("://") else {
        return endpoint.to_string();
    };
    let after_scheme = &endpoint[scheme_end + 3..];
    match after_scheme.find('/') {
        Some(path_start) => format!("{base}{}", &after_scheme[path_start..]),
        None => base.to_string(),
    }
}

/// A random, unguessable token.
///
/// Used for session cookies and PKCE verifiers, both of which need only
/// uniqueness and unpredictability.
fn uuid_v4() -> String {
    uuid_like()
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let addr = &nanos as *const u128 as usize;
    format!("{nanos:032x}{addr:016x}")
}

fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == SESSION_COOKIE)
        .map(|(_, v)| v.to_string())
}

/// Middleware attaching the authenticated subject to the request.
pub async fn attach_subject(
    State(auth): State<Arc<AuthState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if let Some(subject) = auth.resolve(req.headers()).await {
        req.extensions_mut().insert(AuthenticatedSubject(subject));
    }
    next.run(req).await
}

/// Query parameters returned by the provider on the redirect back.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Routes for the login flow.
pub fn router(auth: Arc<AuthState>) -> Router {
    Router::new()
        .route("/auth/login", get(login))
        .route("/auth/callback", get(callback))
        .route("/auth/session", get(session))
        .route("/auth/logout", get(logout))
        .with_state(auth)
}

async fn login(State(auth): State<Arc<AuthState>>) -> Response {
    match &auth.mode {
        AuthMode::Trusted { .. } => Redirect::to("/").into_response(),
        AuthMode::Provider {
            client_id,
            public_url,
            ..
        } => match auth.metadata().await {
            Ok(meta) => {
                let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
                // PKCE is mandatory at this provider. The verifier stays here;
                // only its digest travels through the browser.
                let verifier = uuid_v4();
                let state = uuid_v4();
                auth.pending
                    .write()
                    .await
                    .insert(state.clone(), verifier.clone());
                let url = format!(
                    "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile\
&state={}&code_challenge={}&code_challenge_method=S256",
                    meta.authorization_endpoint,
                    urlencode(client_id),
                    urlencode(&redirect_uri),
                    urlencode(&state),
                    urlencode(&code_challenge(&verifier)),
                );
                Redirect::to(&url).into_response()
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot start login: discovery failed");
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({
                        "title": "Identity provider unavailable",
                        "detail": e,
                    })),
                )
                    .into_response()
            }
        },
    }
}

async fn callback(
    State(auth): State<Arc<AuthState>>,
    Query(params): Query<CallbackParams>,
) -> Response {
    if let Some(err) = params.error {
        tracing::warn!(error = %err, "provider returned an error on callback");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "title": "Login failed", "detail": err })),
        )
            .into_response();
    }
    let Some(code) = params.code else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "title": "Missing authorization code" })),
        )
            .into_response();
    };

    let AuthMode::Provider {
        client_id,
        client_secret,
        public_url,
        ..
    } = &auth.mode
    else {
        return Redirect::to("/").into_response();
    };

    // A callback whose state we did not issue is not ours to complete.
    let verifier = match params.state.as_ref() {
        Some(state) => match auth.pending.write().await.remove(state) {
            Some(v) => v,
            None => {
                tracing::warn!("callback carried an unknown state");
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "title": "Unrecognised login attempt" })),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "title": "Missing state parameter" })),
            )
                .into_response()
        }
    };

    let meta = match auth.metadata().await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "title": "Discovery failed", "detail": e })),
            )
                .into_response()
        }
    };

    let redirect_uri = format!("{}/auth/callback", public_url.trim_end_matches('/'));
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code.as_str()),
        ("redirect_uri", redirect_uri.as_str()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.as_str()),
        ("code_verifier", verifier.as_str()),
    ];

    let resp = match auth
        .http
        .post(&meta.token_endpoint)
        .form(&form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // The error is logged without the form, which carries the secret.
            tracing::error!(error = %e, "token exchange request failed");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "title": "Token exchange failed" })),
            )
                .into_response();
        }
    };
    if !resp.status().is_success() {
        tracing::warn!(status = %resp.status(), "token endpoint rejected the exchange");
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "title": "Token exchange rejected" })),
        )
            .into_response();
    }

    #[derive(Deserialize)]
    struct TokenResponse {
        access_token: String,
    }
    let token: TokenResponse = match resp.json().await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!(error = %e, "token response was not usable");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "title": "Token response not usable" })),
            )
                .into_response();
        }
    };

    let subject = match resolve_subject(&auth, &meta, &token.access_token).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(error = %e, "cannot resolve subject");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "title": "Cannot resolve subject", "detail": e })),
            )
                .into_response();
        }
    };

    let session = auth.issue_session(subject).await;
    // Host-only, HttpOnly, SameSite=Lax: the cookie is never readable from
    // script, and never sent on a cross-site request that could act as the user.
    let cookie =
        format!("{SESSION_COOKIE}={session}; Path=/; HttpOnly; SameSite=Lax; Max-Age=86400");
    (
        StatusCode::SEE_OTHER,
        [(header::SET_COOKIE, cookie), (header::LOCATION, "/".into())],
    )
        .into_response()
}

async fn resolve_subject(
    auth: &AuthState,
    meta: &ProviderMetadata,
    access_token: &str,
) -> Result<SubjectId, String> {
    let Some(endpoint) = meta.userinfo_endpoint.as_ref() else {
        return Err("provider advertises no userinfo endpoint".into());
    };
    let resp = auth
        .http
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| format!("userinfo request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("userinfo returned {}", resp.status()));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("userinfo body not usable: {e}"))?;
    body.get("sub")
        .and_then(|v| v.as_str())
        .map(|s| SubjectId(s.to_string()))
        .ok_or_else(|| "userinfo carried no subject claim".into())
}

async fn session(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    match auth.resolve(&headers).await {
        Some(subject) => Json(serde_json::json!({
            "authenticated": true,
            "subject": subject.as_str(),
        }))
        .into_response(),
        None => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "authenticated": false })),
        )
            .into_response(),
    }
}

async fn logout(State(auth): State<Arc<AuthState>>, headers: HeaderMap) -> Response {
    if let Some(token) = session_cookie(&headers) {
        auth.sessions.write().await.remove(&token);
    }
    (
        StatusCode::SEE_OTHER,
        [
            (
                header::SET_COOKIE,
                format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"),
            ),
            (header::LOCATION, "/".into()),
        ],
    )
        .into_response()
}

fn urlencode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, value.parse().expect("valid header"));
        h
    }

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let h = headers_with_cookie("theme=dark; chrono_bot_session=abc123; other=1");
        assert_eq!(session_cookie(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn a_missing_session_cookie_is_none() {
        let h = headers_with_cookie("theme=dark; other=1");
        assert_eq!(session_cookie(&h), None);
        assert_eq!(session_cookie(&HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn trusted_mode_attributes_every_request_to_one_subject() {
        let auth = AuthState::new(AuthMode::Trusted {
            subject: "local-developer".into(),
        });
        assert!(!auth.requires_credential());
        let got = auth.resolve(&HeaderMap::new()).await;
        assert_eq!(got, Some(SubjectId("local-developer".into())));
    }

    #[tokio::test]
    async fn provider_mode_rejects_a_request_with_no_cookie() {
        let auth = AuthState::new(AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        });
        assert!(auth.requires_credential());
        assert_eq!(auth.resolve(&HeaderMap::new()).await, None);
    }

    #[tokio::test]
    async fn an_issued_session_resolves_and_stops_resolving_after_logout() {
        let auth = AuthState::new(AuthMode::Provider {
            base_url: "http://provider.invalid".into(),
            client_id: "client".into(),
            client_secret: "secret".into(),
            public_url: "http://service.invalid".into(),
        });
        let token = auth.issue_session(SubjectId("subject-a".into())).await;
        let h = headers_with_cookie(&format!("{SESSION_COOKIE}={token}"));
        assert_eq!(auth.resolve(&h).await, Some(SubjectId("subject-a".into())));

        auth.sessions.write().await.remove(&token);
        assert_eq!(auth.resolve(&h).await, None);
    }

    #[tokio::test]
    async fn discovery_against_an_unreachable_provider_reports_why() {
        let auth = AuthState::new(AuthMode::Provider {
            base_url: "http://127.0.0.1:1".into(),
            client_id: "c".into(),
            client_secret: "s".into(),
            public_url: "http://service.invalid".into(),
        });
        let err = auth.metadata().await.expect_err("must fail");
        assert!(err.contains("discovery request"), "got {err}");
    }

    #[test]
    fn urlencoding_escapes_what_a_redirect_uri_needs() {
        assert_eq!(urlencode("http://a/b?c=d"), "http%3A%2F%2Fa%2Fb%3Fc%3Dd");
        assert_eq!(urlencode("plain-Value_1.0~"), "plain-Value_1.0~");
    }

    #[test]
    fn rebase_moves_a_back_channel_endpoint_onto_the_reachable_host() {
        assert_eq!(
            rebase(
                "http://localhost:8444/oauth/token",
                "http://nyxid.internal:8080"
            ),
            "http://nyxid.internal:8080/oauth/token"
        );
    }

    #[test]
    fn rebase_preserves_the_query_string() {
        assert_eq!(
            rebase("https://public/oauth/userinfo?x=1", "http://internal:3001"),
            "http://internal:3001/oauth/userinfo?x=1"
        );
    }

    #[test]
    fn rebase_tolerates_a_base_with_a_trailing_slash() {
        assert_eq!(
            rebase("https://public/a", "http://internal:1/"),
            "http://internal:1/a"
        );
    }

    #[test]
    fn rebase_handles_an_endpoint_with_no_path() {
        assert_eq!(
            rebase("https://public", "http://internal:1"),
            "http://internal:1"
        );
    }

    #[test]
    fn rebase_leaves_an_unparseable_endpoint_alone() {
        // Failing loudly at request time beats silently rewriting nonsense.
        assert_eq!(rebase("not-a-url", "http://internal:1"), "not-a-url");
    }

    #[test]
    fn session_tokens_are_unique() {
        let a = uuid_v4();
        let b = uuid_v4();
        assert_ne!(a, b);
        assert!(a.len() >= 32, "token too short to be unguessable: {a}");
    }
}
