//! Authenticating browsers and services against the identity provider.
//!
//! This service has no user model of its own. It runs the authorization-code
//! flow as a confidential client — the browser never holds the client secret
//! and never calls the token endpoint — and then carries the resulting subject
//! in its own session cookie.
//!
//! Endpoints are discovered from the provider's metadata document rather than
//! hard-coded, so a provider that moves a route does not break this service.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::http::HeaderMap;
pub mod profile;

use layer2_core::{Authorization, SessionId, SubjectId};
use tokio::sync::{Mutex, RwLock};

use token::random_token;

/// Name of the session cookie this service issues.
pub const SESSION_COOKIE: &str = "cma_session";

/// Name issued before the CMA migration, accepted during the compatibility window.
pub(crate) const LEGACY_SESSION_COOKIE: &str = "chrono_bot_session";

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
    /// Mints and reclaims the credential each session presents.
    ///
    /// Optional: without it a session falls back to its owner's own token, and
    /// reaches everything they reach. That is how this service behaved before
    /// credentials were scoped, and it is what a deployment that has not
    /// configured a platform root still gets.
    issuer: RwLock<Option<Arc<keys::KeyIssuer>>>,
    /// The credential minted for each live session.
    ///
    /// Held so a session keeps presenting one value rather than minting a
    /// fresh one on every ask, and so the old one can be reclaimed once its
    /// replacement is in place. The owner is kept alongside because reclaiming
    /// a key needs their token, and by then the session may be gone.
    issued: Arc<RwLock<HashMap<SessionId, Issued>>>,
    /// Credentials no runtime will present, but which are not yet taken back.
    ///
    /// Usually a replaced credential: it is not safe to revoke the moment its
    /// successor is minted because the session may still be presenting it.
    /// Also carries an ended session's credential when its owner token is
    /// momentarily absent. In both cases the sweep waits until revocation can
    /// actually be attempted instead of forgetting a still-valid key.
    superseded: Arc<RwLock<Vec<(String, SubjectId)>>>,
    /// Replacement credentials whose provider mint completed but whose
    /// runtime has not yet confirmed receiving the new value.
    ///
    /// Minting runs in a detached progress task. That task records its result
    /// here even when the sweep awaiting it is cancelled, so the next sweep can
    /// retry delivery without minting another no-expiry key. `retiring` routes
    /// a mint that finishes after release directly onto revoke debt instead.
    reissues: Arc<RwLock<ReissueProgress>>,
    /// Serialises passes so cancellation keeps queued debt and overlapping
    /// sweeps cannot attempt or account for the same key.
    superseded_sweep: Mutex<()>,
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

/// A credential this service minted for one session.
#[derive(Clone)]
pub struct Issued {
    /// The platform's identifier, used to take it back.
    pub id: String,
    /// The value the session presents. Never logged.
    pub value: String,
    /// Whose account it was minted on.
    pub owner: SubjectId,
    /// When it was minted, which is what says when to replace it.
    pub minted_at: std::time::Instant,
}

#[derive(Default)]
struct ReissueProgress {
    pending: HashMap<SessionId, Issued>,
    minting: HashSet<SessionId>,
    retiring: HashSet<SessionId>,
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
            issuer: RwLock::new(None),
            issued: Arc::new(RwLock::new(HashMap::new())),
            superseded: Arc::new(RwLock::new(Vec::new())),
            reissues: Arc::new(RwLock::new(ReissueProgress::default())),
            superseded_sweep: Mutex::new(()),
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
                for token in session_cookies(headers) {
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
    ) -> Vec<(SessionId, SubjectId)> {
        let now = std::time::Instant::now();
        let issued = self.issued.read().await;
        let progress = self.reissues.read().await;
        let mut due: Vec<(SessionId, SubjectId)> = progress
            .pending
            .iter()
            .map(|(s, h)| (s.clone(), h.owner.clone()))
            .collect();
        due.extend(
            issued
                .iter()
                .filter(|&(session, held)| {
                    !progress.pending.contains_key(session)
                        && !progress.minting.contains(session)
                        && !progress.retiring.contains(session)
                        && now.duration_since(held.minted_at) >= lifetime
                })
                .map(|(session, held)| (session.clone(), held.owner.clone())),
        );
        due
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
        session: &SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
    ) -> Option<String> {
        if let Some(pending) = self.reissues.read().await.pending.get(session) {
            return Some(pending.value.clone());
        }

        let issuer = { self.issuer.read().await.clone() }?;
        let owner_token = self.access_token(subject).await?;

        // Reserve while issued is still known to exist. Release and commit use
        // the same issued -> superseded -> reissues order; the mint completion
        // never waits for another lock while holding reissues.
        {
            let issued = self.issued.read().await;
            let mut progress = self.reissues.write().await;
            if let Some(pending) = progress.pending.get(session) {
                return Some(pending.value.clone());
            }
            if !issued.contains_key(session)
                || progress.retiring.contains(session)
                || !progress.minting.insert(session.clone())
            {
                return None;
            }
        }

        let reissues = Arc::clone(&self.reissues);
        let superseded = Arc::clone(&self.superseded);
        let minted_session = session.clone();
        let minted_subject = subject.clone();
        let minted_services = services.to_vec();
        let mint = tokio::spawn(async move {
            let minted = issuer
                .mint(
                    &minted_session,
                    &minted_subject,
                    &owner_token,
                    &minted_services,
                )
                .await;
            // Acquire in the global superseded -> reissues order. In
            // particular, a mint completing after release must not clear its
            // retiring marker before the replacement is visible as retry debt.
            let mut superseded = superseded.write().await;
            let mut progress = reissues.write().await;
            progress.minting.remove(&minted_session);
            let Some(minted) = minted else {
                progress.retiring.remove(&minted_session);
                return None;
            };
            let replacement = Issued {
                id: minted.id,
                value: minted.full_key,
                owner: minted_subject,
                minted_at: std::time::Instant::now(),
            };
            let value = replacement.value.clone();

            if progress.retiring.remove(&minted_session) {
                superseded.push((replacement.id, replacement.owner));
                return None;
            }

            let displaced = progress.pending.insert(minted_session, replacement);
            if let Some(displaced) = displaced {
                superseded.push((displaced.id, displaced.owner));
            }
            Some(value)
        });

        match mint.await {
            Ok(value) => value,
            Err(error) => {
                self.reissues.write().await.minting.remove(session);
                tracing::error!(session_id = %session, %error, "credential replacement task failed");
                None
            }
        }
    }

    /// Promote a pending replacement after its runtime accepted the new value.
    ///
    /// Every mutation happens after all three locks are held in the one global
    /// order. Cancellation while acquiring them leaves the pending replacement
    /// untouched for a later sweep; after acquisition there is no await in the
    /// ownership transfer.
    pub async fn confirm_reissue(&self, session: &SessionId) -> bool {
        let mut issued = self.issued.write().await;
        let mut superseded = self.superseded.write().await;
        let mut progress = self.reissues.write().await;
        let Some(replacement) = progress.pending.remove(session) else {
            return false;
        };
        if progress.retiring.contains(session) || !issued.contains_key(session) {
            superseded.push((replacement.id, replacement.owner));
            return false;
        }
        let old = issued
            .insert(session.clone(), replacement)
            .expect("the issued credential was checked while its lock was held");
        superseded.push((old.id, old.owner));
        true
    }

    /// Take back credentials queued on an earlier pass or release.
    pub async fn take_back_superseded(&self) -> usize {
        let _sweep = self.superseded_sweep.lock().await;
        let Some(issuer) = ({ self.issuer.read().await.clone() }) else {
            return 0;
        };
        let pending = self.superseded.read().await.clone();
        let mut taken = 0;
        for queued in pending {
            let Some(token) = self.access_token(&queued.1).await else {
                continue;
            };
            if issuer.revoke(&queued.0, &token).await {
                let mut owed = self.superseded.write().await;
                if let Some(index) = owed.iter().position(|owed| owed == &queued) {
                    owed.remove(index);
                    taken += 1;
                }
            }
        }
        taken
    }

    /// Take back credentials this deployment minted for sessions that are gone.
    ///
    /// Read from the platform rather than from memory, because the ones worth
    /// finding are exactly those this process has forgotten. Only keys whose
    /// name says this deployment minted them are touched — the account is
    /// shared with other products and other control planes.
    pub async fn reclaim_orphaned_credentials(
        &self,
        service: &Arc<layer2_core::SessionService>,
    ) -> usize {
        let Some(issuer) = ({ self.issuer.read().await.clone() }) else {
            return 0;
        };
        let owners: Vec<SubjectId> = { self.tokens.read().await.keys().cloned().collect() };
        let mut reclaimed = 0;
        for owner in owners {
            let Some(token) = self.access_token(&owner).await else {
                continue;
            };
            let minted = match issuer.minted_here(&token).await {
                Ok(minted) => minted,
                Err(error) => {
                    tracing::warn!(
                        %owner,
                        error = %error,
                        "could not inspect this owner's keys for orphaned credentials"
                    );
                    continue;
                }
            };
            for (id, session) in minted {
                // Claimed, not merely running. A paused session is one its
                // owner kept, and taking its key back leaves them to come
                // back to a session whose credential was revoked underneath
                // them — which is what happened to every stopped session as
                // soon as this process restarted.
                if service.session_claims_credential(&session).await {
                    continue;
                }
                // Held, pending, or still being minted for a session this
                // process is tracking? Then it is not orphaned. Acquire in the
                // global issued -> reissues order so release cannot expose the
                // interval between retiring an in-flight mint and queueing its
                // eventual result.
                let tracked = {
                    let issued = self.issued.read().await;
                    let progress = self.reissues.read().await;
                    issued.contains_key(&session)
                        || progress.pending.contains_key(&session)
                        || progress.minting.contains(&session)
                        || progress.retiring.contains(&session)
                };
                if tracked {
                    continue;
                }
                // Explicit revocation debt has one owner. Letting this sweep
                // delete it would leave its queue entry retrying a 404 forever.
                if self.revoke_is_queued(&id).await {
                    continue;
                }
                if issuer.revoke(&id, &token).await {
                    reclaimed += 1;
                }
            }
        }
        reclaimed
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
pub use tokens::SubjectTokens;

pub(crate) use middleware::session_cookies;
pub(crate) use tokens::ProviderMetadata;

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
