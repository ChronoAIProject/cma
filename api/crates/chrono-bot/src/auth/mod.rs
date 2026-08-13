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
    extract::State,
    http::{header, HeaderMap},
    response::Response,
};
pub mod profile;

use layer1_public::AuthenticatedSubject;
use layer2_core::{Authorization, SubjectId};
use serde::Deserialize;
use tokio::sync::RwLock;

use token::random_token;

/// Name of the session cookie this service issues.
pub const SESSION_COOKIE: &str = "chrono_bot_session";

/// What the provider issued for a signed-in subject.
///
/// The refresh token is the reason this is stored at all: an access token
/// outlives neither a long terminal session nor a long agent turn, and the only
/// alternative to renewing it is interrupting the user.
#[derive(Clone)]
pub struct SubjectTokens {
    /// Currently valid access token.
    pub access_token: String,
    /// Used to obtain the next access token, when the provider issued one.
    pub refresh_token: Option<String>,
    /// When the access token stops being accepted.
    pub expires_at: std::time::Instant,
}

impl std::fmt::Debug for SubjectTokens {
    /// Redacted: no `{:?}` may put a live token in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubjectTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl SubjectTokens {
    /// Whether the access token should be renewed now.
    ///
    /// Renewed early rather than on expiry: a token that dies mid-turn fails a
    /// request the user is waiting on, and the margin costs nothing.
    pub fn needs_refresh(&self, margin: std::time::Duration) -> bool {
        std::time::Instant::now() + margin >= self.expires_at
    }
}

/// Provider metadata, as far as this service uses it.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ProviderMetadata {
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
    /// Provider tokens held per subject, for delegation and renewal.
    tokens: RwLock<HashMap<SubjectId, SubjectTokens>>,
    /// Capabilities resolved at sign-in, per subject.
    authorizations: RwLock<HashMap<SubjectId, Authorization>>,
    /// Display details captured at sign-in, so the interface can show a person
    /// rather than an identifier.
    pub profiles: RwLock<HashMap<SubjectId, profile::Profile>>,
    /// What the provider has said about bearer tokens, briefly.
    ///
    /// Separate from `sessions` because the two are different credentials with
    /// different lifetimes: a cookie is minted here and lasts until sign-out, a
    /// token is minted elsewhere and stops being valid when its issuer says so.
    bearer: bearer::BearerCache,
    /// In-flight authorization attempts: state to PKCE verifier.
    ///
    /// The state parameter is what binds a callback to the login that started
    /// it. Without it a callback from an unrelated flow would be accepted.
    pending: RwLock<HashMap<String, String>>,
    /// Where a sign-in is written down, when this deployment keeps a record.
    ///
    /// Optional because a control plane can be stood up with a fixed subject
    /// and no database in front of it, and in that mode nobody ever signs in —
    /// so there is genuinely nothing to record rather than somewhere missing
    /// to record it.
    records: Option<records::SignInRecords>,
}

/// Derive the PKCE challenge for a verifier, per S256.
fn code_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

impl AuthState {
    /// Build auth state for a mode, and for wherever sign-ins are recorded.
    pub fn new(mode: AuthMode, records: Option<records::SignInRecords>) -> Arc<Self> {
        Arc::new(Self {
            mode,
            records,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
            metadata: RwLock::new(None),
            sessions: RwLock::new(HashMap::new()),
            tokens: RwLock::new(HashMap::new()),
            authorizations: RwLock::new(HashMap::new()),
            profiles: RwLock::new(HashMap::new()),
            bearer: bearer::BearerCache::default(),
            pending: RwLock::new(HashMap::new()),
        })
    }

    /// Whether this service requires a real credential.
    pub fn requires_credential(&self) -> bool {
        matches!(self.mode, AuthMode::Provider { .. })
    }

    /// Capabilities granted in trusted mode.
    ///
    /// Local bring-up has no provider to ask, so the fixed subject is given the
    /// everyday set. Deliberately not the privileged set: a developer running
    /// locally should still have to opt into the admin surface.
    pub fn trusted_authorization() -> Authorization {
        // What an ordinary person can do to their own work, and nothing that
        // reaches anybody else's. A trusted deployment has a single subject,
        // so "their own" is all of it — but the set is still written as a
        // user's rather than an operator's, because this stands in for a
        // signed-in person and a deployment developed against it should behave
        // like the one that is not.
        //
        // Environment profiles are included for the reason permissions.rs
        // gives for granting them alongside session creation: a bundle
        // describes what the session a caller may already start will carry, so
        // a deployment that could start sessions but not describe them would
        // be one where half the product cannot be used at all.
        Authorization::from_capabilities([
            layer2_core::Capability::CreateSession,
            layer2_core::Capability::UseOwnSessions,
            layer2_core::Capability::TerminateOwnSession,
            layer2_core::Capability::ManageOwnEnvironmentProfiles,
        ])
    }

    /// Resolve the subject for a request, if any.
    pub async fn resolve(&self, headers: &HeaderMap) -> Option<SubjectId> {
        match &self.mode {
            AuthMode::Trusted { subject } => Some(SubjectId(subject.clone())),
            AuthMode::Provider { .. } => {
                if let Some(token) = session_cookie(headers) {
                    if let Some(subject) = self.sessions.read().await.get(&token).cloned() {
                        return Some(subject);
                    }
                }
                // A server client has no cookie jar. Its token is checked with
                // the provider rather than read here, so a token this service
                // did not issue — and one that has since been withdrawn — is
                // refused rather than believed.
                let token = bearer::bearer_token(headers)?;
                self.subject_for_bearer(&token).await
            }
        }
    }

    /// Who a bearer token belongs to, asking the provider when necessary.
    ///
    /// The provider is the only thing that can answer this. Decoding the token
    /// here would accept anything shaped like one, and would keep accepting it
    /// after it had been revoked.
    async fn subject_for_bearer(&self, token: &str) -> Option<SubjectId> {
        if let Some((subject, authorization, profile)) = self.bearer.get(token).await {
            self.adopt_bearer(&subject, authorization, profile, token)
                .await;
            return Some(subject);
        }
        let meta = self.metadata().await.ok()?;
        match crate::auth::flow::resolve_subject(self, &meta, token).await {
            Ok((subject, permissions, profile)) => {
                let authorization = crate::permissions::capabilities_from_permissions(&permissions);
                self.bearer
                    .remember(
                        token,
                        subject.clone(),
                        authorization.clone(),
                        profile.clone(),
                    )
                    .await;
                self.adopt_bearer(&subject, authorization, profile, token)
                    .await;
                Some(subject)
            }
            Err(e) => {
                // Not an error worth a stack trace: an expired or withdrawn
                // token is an ordinary thing for a client to present.
                tracing::debug!(error = %e, "a bearer token was not accepted");
                None
            }
        }
    }

    /// Make a bearer caller indistinguishable from a signed-in one downstream.
    ///
    /// Everything past the middleware reads capabilities, a profile and a
    /// delegated credential from the same places whichever way the caller
    /// arrived. Keeping the token as the delegated credential is what lets a
    /// session started by a server act on that caller's behalf, exactly as one
    /// started from a browser does.
    async fn adopt_bearer(
        &self,
        subject: &SubjectId,
        authorization: Authorization,
        profile: crate::auth::profile::Profile,
        token: &str,
    ) {
        self.authorizations
            .write()
            .await
            .insert(subject.clone(), authorization);
        self.profiles.write().await.insert(subject.clone(), profile);
        let mut tokens = self.tokens.write().await;
        let entry = tokens
            .entry(subject.clone())
            .or_insert_with(|| SubjectTokens {
                access_token: token.to_string(),
                refresh_token: None,
                // Held for as long as this service will reuse the answer about it.
                // The provider decides when the token really expires; this only
                // says when to stop assuming it has not been withdrawn.
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(60),
            });
        // Only when this service is not already holding a better one: a token
        // it minted itself can be renewed, and a client's cannot.
        if entry.refresh_token.is_none() {
            entry.access_token = token.to_string();
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

    /// Store the tokens issued for a subject.
    pub async fn store_tokens(&self, subject: &SubjectId, tokens: SubjectTokens) {
        self.tokens.write().await.insert(subject.clone(), tokens);
    }

    /// The subject's current access token, if one is held.
    pub async fn access_token(&self, subject: &SubjectId) -> Option<String> {
        self.tokens
            .read()
            .await
            .get(subject)
            .map(|t| t.access_token.clone())
    }

    /// Subjects whose access token is close enough to expiry to renew.
    pub async fn subjects_needing_refresh(
        &self,
        margin: std::time::Duration,
    ) -> Vec<(SubjectId, String)> {
        self.tokens
            .read()
            .await
            .iter()
            .filter(|(_, t)| t.needs_refresh(margin))
            .filter_map(|(s, t)| t.refresh_token.clone().map(|r| (s.clone(), r)))
            .collect()
    }

    async fn issue_session(&self, subject: SubjectId) -> String {
        let token = random_token();
        self.sessions
            .write()
            .await
            .insert(token.clone(), subject.clone());
        tracing::info!(subject = %subject, "session issued");
        token
    }
}

/// Supply the core with an owner's credential when the owner is absent.
///
/// The core recovers sessions on its own — after a restart, or when a runtime
/// has to be replaced — and a recovered session that presents no credential is
/// a session whose every model call fails. So the credential is fetched from
/// wherever it is currently held rather than being carried along from the
/// request that happened to create the session.
#[async_trait::async_trait]
impl layer2_core::CredentialSource for AuthState {
    async fn credential_for(&self, subject: &SubjectId) -> Option<String> {
        self.access_token(subject).await
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
        // The credential rides separately from identity: nearly every handler
        // needs the subject, almost none needs a credential, so taking one has
        // to be deliberate.
        if let Some(token) = auth.access_token(&subject).await {
            req.extensions_mut()
                .insert(layer1_public::auth::DelegatedCredential(token));
        }
        // The same resolution the identity endpoint reports, so what a client
        // was told it may do and what this request is allowed to do cannot
        // drift apart.
        req.extensions_mut()
            .insert(auth.effective_authorization(&subject).await);
        req.extensions_mut().insert(AuthenticatedSubject(subject));
    }
    next.run(req).await
}

pub mod bearer;
pub mod capabilities;
pub mod entitlements;
pub mod flow;
pub mod records;
pub mod refresh;
mod token;

pub use records::SignInRecords;

pub use flow::router;

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
        let auth = AuthState::new(
            AuthMode::Trusted {
                subject: "local-developer".into(),
            },
            None,
        );
        assert!(!auth.requires_credential());
        let got = auth.resolve(&HeaderMap::new()).await;
        assert_eq!(got, Some(SubjectId("local-developer".into())));
    }

    #[tokio::test]
    async fn provider_mode_rejects_a_request_with_no_cookie() {
        let auth = AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "secret".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        );
        assert!(auth.requires_credential());
        assert_eq!(auth.resolve(&HeaderMap::new()).await, None);
    }

    #[tokio::test]
    async fn an_issued_session_resolves_and_stops_resolving_after_logout() {
        let auth = AuthState::new(
            AuthMode::Provider {
                base_url: "http://provider.invalid".into(),
                client_id: "client".into(),
                client_secret: "secret".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        );
        let token = auth.issue_session(SubjectId("subject-a".into())).await;
        let h = headers_with_cookie(&format!("{SESSION_COOKIE}={token}"));
        assert_eq!(auth.resolve(&h).await, Some(SubjectId("subject-a".into())));

        auth.sessions.write().await.remove(&token);
        assert_eq!(auth.resolve(&h).await, None);
    }

    #[tokio::test]
    async fn discovery_against_an_unreachable_provider_reports_why() {
        let auth = AuthState::new(
            AuthMode::Provider {
                base_url: "http://127.0.0.1:1".into(),
                client_id: "c".into(),
                client_secret: "s".into(),
                public_url: "http://service.invalid".into(),
            },
            None,
        );
        let err = auth.metadata().await.expect_err("must fail");
        assert!(err.contains("discovery request"), "got {err}");
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
}
