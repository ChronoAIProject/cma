//! Authenticating browsers and services against the identity provider.
//!
//! This service has no user model of its own. It runs the authorization-code
//! flow as a confidential client — the browser never holds the client secret
//! and never calls the token endpoint — and then carries the resulting subject
//! in its own session cookie.
//!
//! Endpoints are discovered from the provider's metadata document rather than
//! hard-coded, so a provider that moves a route does not break this service.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::http::HeaderMap;
pub mod persistence;
pub mod profile;

use layer2_core::store::StoreError;
use layer2_core::{Authorization, SubjectId};
use tokio::sync::{Mutex, RwLock};

use persistence::{
    AuthStore, BrowserSession, DurableSubjectTokens, SecretDigest, SubjectAuthState,
};
#[cfg(test)]
use persistence::{MemoryAuthStore, SecretValue};
use token::random_token;

/// Name of the session cookie this service issues.
pub const SESSION_COOKIE: &str = "cma_session";

/// Name issued before the CMA migration, accepted during the compatibility window.
pub(crate) const LEGACY_SESSION_COOKIE: &str = "chrono_bot_session";

const BROWSER_SESSION_DOMAIN: &str = "browser-session";
const BROWSER_SESSION_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

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
    /// Durable authority shared by every replica.
    store: Arc<dyn AuthStore>,
    /// Mints and reclaims the credential each session presents.
    ///
    /// Optional: without it a session falls back to its owner's own token, and
    /// reaches everything they reach. That is how this service behaved before
    /// credentials were scoped, and it is what a deployment that has not
    /// configured a platform root still gets.
    issuer: RwLock<Option<Arc<keys::KeyIssuer>>>,
    /// Avoid duplicate provider deletes within one replica. Durable debt and
    /// generation CAS remain the authority across replicas.
    credential_sweep: Mutex<()>,
    /// What the provider has said about bearer tokens, briefly.
    ///
    /// Separate from `sessions` because the two are different credentials with
    /// different lifetimes: a cookie is minted here and lasts until sign-out, a
    /// token is minted elsewhere and stops being valid when its issuer says so.
    bearer: bearer::BearerCache,
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
    #[cfg(test)]
    pub fn new(mode: AuthMode, records: Option<records::SignInRecords>) -> Arc<Self> {
        Self::with_store(mode, records, Arc::new(MemoryAuthStore::default()))
    }

    /// Build auth state over the durable authority shared by this deployment.
    pub fn with_store(
        mode: AuthMode,
        records: Option<records::SignInRecords>,
        store: Arc<dyn AuthStore>,
    ) -> Arc<Self> {
        Arc::new(Self {
            mode,
            records,
            store,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("http client builds"),
            metadata: RwLock::new(None),
            issuer: RwLock::new(None),
            credential_sweep: Mutex::new(()),
            bearer: bearer::BearerCache::default(),
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
    pub async fn resolve(&self, headers: &HeaderMap) -> Result<Option<SubjectId>, StoreError> {
        match &self.mode {
            AuthMode::Trusted { subject } => Ok(Some(SubjectId(subject.clone()))),
            AuthMode::Provider { .. } => {
                for token in session_cookies(headers) {
                    let id = SecretDigest::identify(BROWSER_SESSION_DOMAIN, &token);
                    if let Some(session) = self.store.browser_session(&id).await? {
                        if session.expires_at > SystemTime::now() {
                            return Ok(Some(session.subject));
                        }
                        // TTL deletion is eventual. An expired record is never
                        // accepted, and best-effort cleanup does not change the
                        // authentication answer.
                        if let Err(error) = self.store.delete_browser_session(&id).await {
                            tracing::warn!(
                                error = %error,
                                "could not remove an expired browser session"
                            );
                        }
                    }
                }
                // A server client has no cookie jar. Its token is checked with
                // the provider rather than read here, so a token this service
                // did not issue — and one that has since been withdrawn — is
                // refused rather than believed.
                let Some(token) = bearer::bearer_token(headers) else {
                    return Ok(None);
                };
                self.subject_for_bearer(&token).await
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
        meta.introspection_endpoint = meta
            .introspection_endpoint
            .as_deref()
            .map(|e| rebase(e, &base));

        *self.metadata.write().await = Some(meta.clone());
        tracing::info!(
            %url,
            authorization_endpoint = %meta.authorization_endpoint,
            token_endpoint = %meta.token_endpoint,
            "provider metadata discovered"
        );
        Ok(meta)
    }

    /// Give this service the means to mint per-session credentials.
    ///
    /// Set after construction because the platform root is configuration and
    /// this state is built before it is read. Absent leaves every session on
    /// its owner's own token, which is the behaviour that predates scoping.
    pub async fn use_issuer(&self, issuer: Arc<keys::KeyIssuer>) {
        *self.issuer.write().await = Some(issuer);
    }

    /// Whether sessions hold credentials of their own rather than their owner's.
    ///
    /// True once an issuer is configured, which is what `use_issuer` says in
    /// the other direction: absent leaves every session on its owner's own
    /// token, the behaviour that predates scoping.
    ///
    /// Asked because renewing the owner's token means two different things
    /// depending on the answer. Where sessions hold their own keys it is this
    /// service's own ability to mint and revoke being kept alive, and pushing
    /// it into a session would hand back the whole identity those keys exist
    /// to replace. Where they do not, the renewed token *is* what the sessions
    /// run on and must reach them.
    pub async fn sessions_hold_own_credentials(&self) -> bool {
        self.issuer.read().await.is_some()
    }

    /// Sessions whose credential has been held longer than `lifetime`.
    ///
    /// Age rather than a stated expiry, because the platform does not return
    /// or request one when it mints. The caller chooses a local rotation
    /// cadence independently of how long the provider accepts the key.
    pub async fn sessions_due_for_reissue(
        &self,
        lifetime: std::time::Duration,
    ) -> Result<Vec<(layer2_core::SessionId, SubjectId)>, StoreError> {
        let now = SystemTime::now();
        Ok(self
            .store
            .credentials()
            .await?
            .into_iter()
            .filter(|state| !state.retired && state.lease.is_none())
            .filter_map(|state| {
                if let Some(pending) = state.pending {
                    return Some((state.session, pending.owner));
                }
                let current = state.current?;
                let age = now
                    .duration_since(current.issued_at)
                    .unwrap_or(Duration::ZERO);
                (age >= lifetime).then_some((state.session, current.owner))
            })
            .collect())
    }

    /// Mint, or resume delivering, a fresh credential for one session.
    ///
    /// The provider request belongs to a detached task. Cancelling the caller
    /// only stops it waiting: a successful mint is still recorded in `pending`
    /// for the next sweep, rather than becoming an unowned no-expiry key. The
    /// current credential remains issued until [`Self::confirm_reissue`] says
    /// the runtime accepted this value.
    pub async fn reissue(
        &self,
        session: &layer2_core::SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential(session, subject, services, true)
            .await
    }

    /// Promote a pending replacement after its runtime accepted the new value.
    ///
    /// A generation compare-and-swap moves the old current value to durable
    /// revoke debt in the same transition that promotes the replacement.
    pub async fn confirm_reissue(
        &self,
        session: &layer2_core::SessionId,
    ) -> Result<bool, StoreError> {
        self.promote_pending_credential(session).await
    }

    /// Take back credentials queued on an earlier pass or release.
    pub async fn revoke_retired_credentials(&self) -> Result<usize, StoreError> {
        self.revoke_credential_debt().await
    }

    /// Take back credentials this deployment minted for sessions that are gone.
    ///
    /// Provider listings remain the reconciliation backstop for credentials
    /// minted before durable custody existed, and for the response-loss window
    /// tracked by #528. Only keys whose name says this deployment minted them
    /// are touched; the account is shared with other control planes.
    pub async fn reclaim_orphaned_credentials(
        &self,
        service: &Arc<layer2_core::SessionService>,
    ) -> Result<usize, StoreError> {
        self.reclaim_unrecorded_credentials(service).await
    }

    /// Replace a subject fact with revision-CAS, retrying a concurrent writer.
    async fn replace_subject_fact<F>(
        &self,
        subject: &SubjectId,
        mut update: F,
    ) -> Result<(), StoreError>
    where
        F: FnMut(&mut SubjectAuthState),
    {
        loop {
            let current = self.store.subject(subject).await?;
            let mut next = current.clone().unwrap_or_else(|| SubjectAuthState {
                subject: subject.clone(),
                revision: 0,
                tokens: None,
                refresh_lease: None,
                authorization: Authorization::none(),
                profile: profile::Profile::default(),
            });
            update(&mut next);
            let written = match current {
                Some(current) => {
                    next.revision = current.revision.checked_add(1).ok_or_else(|| {
                        StoreError::Malformed("subject auth revision exhausted".into())
                    })?;
                    self.store.replace_subject(current.revision, next).await?
                }
                None => self.store.insert_subject(next).await?,
            };
            if written {
                return Ok(());
            }
        }
    }

    /// Read one subject fact from the durable authority.
    pub(super) async fn subject_fact(
        &self,
        subject: &SubjectId,
    ) -> Result<Option<SubjectAuthState>, StoreError> {
        self.store.subject(subject).await
    }

    /// Persist a completed sign-in as one indivisible subject fact.
    async fn store_sign_in(
        &self,
        subject: &SubjectId,
        tokens: DurableSubjectTokens,
        authorization: Authorization,
        profile: profile::Profile,
    ) -> Result<(), StoreError> {
        self.replace_subject_fact(subject, |state| {
            state.tokens = Some(tokens.clone());
            state.refresh_lease = None;
            state.authorization = authorization.clone();
            state.profile = profile.clone();
        })
        .await
    }

    /// Store the tokens issued for a subject.
    #[cfg(test)]
    pub async fn store_tokens(
        &self,
        subject: &SubjectId,
        tokens: SubjectTokens,
    ) -> Result<(), StoreError> {
        let remaining = tokens
            .expires_at
            .saturating_duration_since(std::time::Instant::now());
        let durable = DurableSubjectTokens {
            access_token: SecretValue::new(tokens.access_token),
            refresh_token: tokens.refresh_token.map(SecretValue::new),
            expires_at: SystemTime::now() + remaining,
        };
        self.replace_subject_fact(subject, |state| {
            state.tokens = Some(durable.clone());
            state.refresh_lease = None;
        })
        .await
    }

    /// Revoke one raw browser cookie without ever storing or logging it.
    async fn revoke_session(&self, token: &str) -> Result<(), StoreError> {
        self.store
            .delete_browser_session(&SecretDigest::identify(BROWSER_SESSION_DOMAIN, token))
            .await
    }

    /// The subject's current access token, if one is held.
    pub async fn access_token(&self, subject: &SubjectId) -> Result<Option<String>, StoreError> {
        Ok(self
            .store
            .subject(subject)
            .await?
            .and_then(|state| state.tokens)
            .filter(|tokens| tokens.expires_at > SystemTime::now())
            .map(|tokens| tokens.access_token.into_inner()))
    }

    /// Subjects whose access token is close enough to expiry to renew.
    pub async fn subjects_needing_refresh(
        &self,
        margin: std::time::Duration,
    ) -> Result<Vec<(SubjectId, String)>, StoreError> {
        let refresh_at = SystemTime::now() + margin;
        Ok(self
            .store
            .subjects()
            .await?
            .into_iter()
            .filter_map(|state| {
                if state
                    .refresh_lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at > SystemTime::now())
                {
                    return None;
                }
                let tokens = state.tokens?;
                if tokens.expires_at > refresh_at {
                    return None;
                }
                tokens
                    .refresh_token
                    .map(|refresh| (state.subject, refresh.into_inner()))
            })
            .collect())
    }

    async fn issue_session(&self, subject: SubjectId) -> Result<String, StoreError> {
        let token = random_token();
        self.store
            .put_browser_session(BrowserSession {
                id: SecretDigest::identify(BROWSER_SESSION_DOMAIN, &token),
                subject: subject.clone(),
                expires_at: SystemTime::now() + BROWSER_SESSION_LIFETIME,
            })
            .await?;
        tracing::info!(subject = %subject, "session issued");
        Ok(token)
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

pub mod bearer;
mod delegation;
mod identity;
mod middleware;
mod tokens;

pub use middleware::attach_subject;
#[cfg(test)]
pub use tokens::SubjectTokens;

pub(crate) use middleware::session_cookies;
pub(crate) use tokens::ProviderMetadata;

#[cfg(test)]
pub mod capabilities;
pub mod entitlements;
pub mod flow;
pub mod keys;
mod permissions;
pub mod records;
pub mod refresh;
mod session_routes;
mod token;

pub use records::SignInRecords;

pub use flow::router;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
