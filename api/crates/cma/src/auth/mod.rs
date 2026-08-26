//! Authenticating browsers and services against the identity provider.
//!
//! This service has no user model of its own. It runs the authorization-code
//! flow as a confidential client — the browser never holds the client secret
//! and never calls the token endpoint — and then carries the resulting subject
//! in its own session cookie.
//!
//! Endpoints are discovered from the provider's metadata document rather than
//! hard-coded, so a provider that moves a route does not break this service.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use axum::http::HeaderMap;
pub mod persistence;
pub mod profile;

use layer2_core::store::StoreError;
use layer2_core::{Authorization, SessionId, SubjectId};
use tokio::sync::{Mutex, RwLock};

use persistence::{
    AuthStore, AuthSweepItem, BrowserSession, CredentialLeaseCursor, CredentialMutationCursor,
    CredentialReissueCursor, DurableSubjectTokens, RevokeDebtCursor, SecretDigest,
    SubjectAuthState, SubjectOwnerCursor, SubjectRefreshCursor,
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
/// Maximum actionable authentication records selected by one renewal lane.
pub(super) const AUTH_SWEEP_BATCH_SIZE: usize = 100;
/// Existing custody lanes plus one independently reserved authorization lane.
pub(super) const AUTH_RENEWAL_PASS_BUDGET: usize =
    5 * AUTH_SWEEP_BATCH_SIZE + AUTHORIZATION_CHECK_LANE_BUDGET;
pub(super) const AUTH_SWEEP_TWO_LANE_BATCH_SIZE: usize = AUTH_SWEEP_BATCH_SIZE / 2;
const AUTH_SWEEP_THREE_LANE_BATCH_SIZE: usize = AUTH_SWEEP_BATCH_SIZE / 3;
#[cfg(test)]
const AUTH_SWEEP_LEASE_LANE_BATCH_SIZE: usize =
    AUTH_SWEEP_BATCH_SIZE - (2 * AUTH_SWEEP_THREE_LANE_BATCH_SIZE);
/// Maximum external operations available to one selected recurring reissue.
///
/// The service-scope lookup can read both the session and its legacy
/// environment profile. The remaining thirteen units cover custody, provider
/// resolution and minting, runtime delivery, and pending promotion.
pub(super) const AUTH_REISSUE_ATTEMPT_BUDGET: usize = 15;
/// Maximum external operations available to one selected recurring refresh.
pub(super) const AUTH_REFRESH_ATTEMPT_BUDGET: usize = 10;
/// One selected authorization check can resolve twice around one CAS loss.
///
/// The first attempt spends metadata, userinfo, optional introspection, and a
/// two-operation durable replacement. A contended retry additionally rereads
/// the winner before resolving against its token generation.
pub(super) const AUTHORIZATION_CHECK_ATTEMPT_BUDGET: usize = 11;
/// One hundred work pages plus one terminating/backlog-probe query (10,000 subjects).
pub(super) const AUTHORIZATION_CHECK_PAGE_BUDGET: usize = 101;
pub(super) const AUTHORIZATION_CHECK_LANE_BUDGET: usize = AUTHORIZATION_CHECK_PAGE_BUDGET
    + (AUTHORIZATION_CHECK_PAGE_BUDGET - 1)
        * AUTH_SWEEP_BATCH_SIZE
        * AUTHORIZATION_CHECK_ATTEMPT_BUDGET;

struct AuthPassBudgetState {
    remaining: AtomicUsize,
    used: AtomicUsize,
}

/// One pass-wide operation counter shared by every independently fair lane.
pub(super) struct AuthPassBudget {
    state: Arc<AuthPassBudgetState>,
}

/// A bounded view of the shared pass counter carried into one lane or task.
#[derive(Clone)]
pub(super) struct AuthOperationBudget {
    state: Arc<AuthPassBudgetState>,
    remaining: Arc<AtomicUsize>,
}

impl AuthPassBudget {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            state: Arc::new(AuthPassBudgetState {
                remaining: AtomicUsize::new(limit),
                used: AtomicUsize::new(0),
            }),
        }
    }

    pub(super) fn lane(&self, limit: usize) -> AuthOperationBudget {
        AuthOperationBudget {
            state: Arc::clone(&self.state),
            remaining: Arc::new(AtomicUsize::new(limit)),
        }
    }

    pub(super) fn used(&self) -> usize {
        self.state.used.load(Ordering::SeqCst)
    }
}

impl AuthOperationBudget {
    pub(super) fn standalone(limit: usize) -> Self {
        AuthPassBudget::new(limit).lane(limit)
    }

    pub(super) fn remaining(&self) -> usize {
        self.remaining
            .load(Ordering::SeqCst)
            .min(self.state.remaining.load(Ordering::SeqCst))
    }

    pub(super) fn take(&self, amount: usize) -> bool {
        if amount == 0 {
            return true;
        }
        if !subtract_atomic(&self.remaining, amount) {
            return false;
        }
        if !subtract_atomic(&self.state.remaining, amount) {
            self.remaining.fetch_add(amount, Ordering::SeqCst);
            return false;
        }
        self.state.used.fetch_add(amount, Ordering::SeqCst);
        true
    }

    pub(super) fn refund(&self, amount: usize) {
        if amount == 0 {
            return;
        }
        self.remaining.fetch_add(amount, Ordering::SeqCst);
        self.state.remaining.fetch_add(amount, Ordering::SeqCst);
        self.state.used.fetch_sub(amount, Ordering::SeqCst);
    }
}

fn subtract_atomic(value: &AtomicUsize, amount: usize) -> bool {
    value
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current.checked_sub(amount)
        })
        .is_ok()
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
        /// HTTPS origins also require the Secure attribute on browser cookies;
        /// explicit HTTP remains available for local development.
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

#[derive(Clone)]
struct SweepEpoch<C> {
    cutoff: Option<SystemTime>,
    through_sequence: u64,
    after: Option<C>,
}

#[derive(Clone, Default)]
struct RefreshSweepEpochs {
    unleased: Option<SweepEpoch<SubjectRefreshCursor>>,
    expired_lease: Option<SweepEpoch<SubjectRefreshCursor>>,
}

#[derive(Clone, Default)]
struct ReissueSweepEpochs {
    pending: Option<SweepEpoch<CredentialMutationCursor>>,
    current: Option<SweepEpoch<CredentialReissueCursor>>,
    expired_lease: Option<SweepEpoch<CredentialLeaseCursor>>,
}

#[derive(Clone, Default)]
struct OrphanReconciliationSweep {
    owners: Option<SweepEpoch<SubjectOwnerCursor>>,
    current_owner: Option<OrphanOwnerSweep>,
}

#[derive(Clone)]
struct OrphanOwnerSweep {
    subject: SubjectId,
    remaining_credentials: Option<std::collections::VecDeque<(String, SessionId)>>,
    owners_after_completion: Option<SweepEpoch<SubjectOwnerCursor>>,
}

#[derive(Debug)]
pub struct SweepBatch<T> {
    pub(super) work: Vec<T>,
    pub(super) first_error: Option<StoreError>,
}

#[cfg(test)]
impl<T> SweepBatch<T> {
    pub(super) fn len(&self) -> usize {
        self.work.len()
    }

    pub(super) fn iter(&self) -> std::slice::Iter<'_, T> {
        self.work.iter()
    }

    pub(super) fn first(&self) -> Option<&T> {
        self.work.first()
    }

    pub(super) fn last(&self) -> Option<&T> {
        self.work.last()
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        self.work.pop()
    }
}

impl<T> IntoIterator for SweepBatch<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.work.into_iter()
    }
}

#[cfg(test)]
impl<T: PartialEq> PartialEq<Vec<T>> for SweepBatch<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.work == *other
    }
}

fn remember_sweep_item_error(slot: &mut Option<StoreError>, error: StoreError, operation: &str) {
    tracing::warn!(error = %error, operation, "bounded authentication sweep item failed");
    if slot.is_none() {
        *slot = Some(error);
    }
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
    /// Fairness-only page positions. Durable facts, leases, and CAS remain the
    /// authority; losing these hints merely restarts a bounded scan at the
    /// beginning after a process restart.
    refresh_sweep_epochs: Mutex<RefreshSweepEpochs>,
    active_credential_sweep_epoch:
        Mutex<Option<SweepEpoch<persistence::CredentialReconciliationCursor>>>,
    orphan_reconciliation_sweep: Mutex<OrphanReconciliationSweep>,
    reissue_sweep_epochs: Mutex<ReissueSweepEpochs>,
    revoke_debt_sweep_epoch: Mutex<Option<SweepEpoch<RevokeDebtCursor>>>,
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

/// Authentication facts established for exactly one incoming credential.
///
/// Bearer-token facts never pass through the durable browser-subject record:
/// two tokens may name the same subject while carrying different grants and
/// delegated authority. Keeping the complete result together is what lets the
/// middleware attach one internally consistent caller to the request.
pub(super) struct RequestAuth {
    pub(super) subject: SubjectId,
    pub(super) authorization: Authorization,
    pub(super) profile: profile::Profile,
    pub(super) delegated_credential: Option<String>,
    pub(super) credential_is_request_bound: bool,
}

/// A token generation observed by one renewal sweep.
pub(super) struct DueSubjectRefresh {
    pub(super) subject: SubjectId,
    pub(super) refresh_token: String,
    pub(super) incarnation: String,
    pub(super) token_generation: u64,
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
            refresh_sweep_epochs: Mutex::new(RefreshSweepEpochs::default()),
            active_credential_sweep_epoch: Mutex::new(None),
            orphan_reconciliation_sweep: Mutex::new(OrphanReconciliationSweep::default()),
            reissue_sweep_epochs: Mutex::new(ReissueSweepEpochs::default()),
            revoke_debt_sweep_epoch: Mutex::new(None),
            bearer: bearer::BearerCache::default(),
        })
    }

    /// Whether this service requires a real credential.
    pub fn requires_credential(&self) -> bool {
        matches!(self.mode, AuthMode::Provider { .. })
    }

    fn secure_cookies(&self) -> bool {
        let AuthMode::Provider { public_url, .. } = &self.mode else {
            return false;
        };
        reqwest::Url::parse(public_url).is_ok_and(|url| url.scheme() == "https")
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

    /// Resolve all authentication facts for one request, if it carries a credential.
    pub(super) async fn resolve_request(
        &self,
        headers: &HeaderMap,
    ) -> Result<Option<RequestAuth>, StoreError> {
        match &self.mode {
            AuthMode::Trusted { subject } => Ok(Some(RequestAuth {
                subject: SubjectId(subject.clone()),
                authorization: Self::trusted_authorization(),
                profile: profile::Profile::default(),
                delegated_credential: None,
                credential_is_request_bound: false,
            })),
            AuthMode::Provider { .. } => {
                for token in session_cookies(headers) {
                    let id = SecretDigest::identify(BROWSER_SESSION_DOMAIN, &token);
                    if let Some(session) = self.store.browser_session(&id).await? {
                        if session.expires_at > SystemTime::now() {
                            let subject = session.subject;
                            let Some(fact) = self.store.subject(&subject).await? else {
                                return Ok(Some(RequestAuth {
                                    subject,
                                    authorization: Authorization::none(),
                                    profile: profile::Profile::default(),
                                    delegated_credential: None,
                                    credential_is_request_bound: false,
                                }));
                            };
                            let delegated_credential = fact
                                .tokens
                                .filter(|tokens| tokens.expires_at > SystemTime::now())
                                .map(|tokens| tokens.access_token.into_inner());
                            return Ok(Some(RequestAuth {
                                subject,
                                authorization: fact.authorization,
                                profile: fact.profile,
                                delegated_credential,
                                credential_is_request_bound: false,
                            }));
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
                self.request_for_bearer(token).await
            }
        }
    }

    /// Resolve only identity for callers that do not consume request authority.
    #[cfg(test)]
    pub async fn resolve(&self, headers: &HeaderMap) -> Result<Option<SubjectId>, StoreError> {
        Ok(self
            .resolve_request(headers)
            .await?
            .map(|resolved| resolved.subject))
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

    /// Advance one bounded lane of durable authentication schema maintenance.
    pub(super) async fn maintain_schema(
        &self,
        budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        let reserved = budget.remaining();
        if reserved == 0 || !budget.take(reserved) {
            return Ok(0);
        }
        match self.store.maintain_auth_schema(reserved).await {
            Ok(used) if used <= reserved => {
                budget.refund(reserved - used);
                Ok(used)
            }
            Ok(_) => Err(StoreError::Malformed(
                "authentication schema maintenance exceeded its operation allowance".into(),
            )),
            Err(error) => Err(error),
        }
    }

    /// Sessions whose credential has been held longer than `lifetime`.
    ///
    /// Age rather than a stated expiry, because the platform does not return
    /// or request one when it mints. The caller chooses a local rotation
    /// cadence independently of how long the provider accepts the key.
    #[cfg(test)]
    pub async fn sessions_due_for_reissue(
        &self,
        lifetime: std::time::Duration,
    ) -> Result<SweepBatch<layer2_core::SessionId>, StoreError> {
        let now = SystemTime::now();
        let issued_before = now.checked_sub(lifetime).unwrap_or(SystemTime::UNIX_EPOCH);
        let mut epochs = self.reissue_sweep_epochs.lock().await;
        let mut proposed = epochs.clone();
        if proposed.pending.is_none()
            || proposed.current.is_none()
            || proposed.expired_lease.is_none()
        {
            let through_sequence = self.store.auth_sweep_high_water().await?;
            proposed.pending.get_or_insert(SweepEpoch {
                cutoff: None,
                through_sequence,
                after: None,
            });
            proposed.current.get_or_insert(SweepEpoch {
                cutoff: Some(issued_before),
                through_sequence,
                after: None,
            });
            proposed.expired_lease.get_or_insert(SweepEpoch {
                cutoff: Some(now),
                through_sequence,
                after: None,
            });
        }

        let mut pending = if let Some(epoch) = proposed.pending.clone() {
            let states = self
                .store
                .pending_credentials_due_for_reissue(
                    epoch.after.as_ref(),
                    epoch.through_sequence,
                    AUTH_SWEEP_THREE_LANE_BATCH_SIZE,
                )
                .await?;
            proposed.pending = finish_sweep_page(
                &states,
                AUTH_SWEEP_THREE_LANE_BATCH_SIZE,
                &epoch,
                pending_reissue_position,
            );
            states
        } else {
            Vec::new()
        };
        let pending_lane_full = pending.len() == AUTH_SWEEP_THREE_LANE_BATCH_SIZE;

        let mut current = if let Some(epoch) = proposed.current.clone() {
            let cutoff = epoch.cutoff.expect("current reissue epoch has a cutoff");
            let states = self
                .store
                .current_credentials_due_for_reissue(
                    cutoff,
                    epoch.after.as_ref(),
                    epoch.through_sequence,
                    AUTH_SWEEP_THREE_LANE_BATCH_SIZE,
                )
                .await?;
            proposed.current = finish_sweep_page(
                &states,
                AUTH_SWEEP_THREE_LANE_BATCH_SIZE,
                &epoch,
                current_reissue_position,
            );
            states
        } else {
            Vec::new()
        };
        let current_lane_full = current.len() == AUTH_SWEEP_THREE_LANE_BATCH_SIZE;

        let mut expired_leases = if let Some(epoch) = proposed.expired_lease.clone() {
            let cutoff = epoch
                .cutoff
                .expect("expired credential lease epoch has a cutoff");
            let states = self
                .store
                .credentials_with_expired_mint_leases(
                    cutoff,
                    epoch.after.as_ref(),
                    epoch.through_sequence,
                    AUTH_SWEEP_LEASE_LANE_BATCH_SIZE,
                )
                .await?;
            proposed.expired_lease = finish_sweep_page(
                &states,
                AUTH_SWEEP_LEASE_LANE_BATCH_SIZE,
                &epoch,
                expired_credential_lease_position,
            );
            states
        } else {
            Vec::new()
        };
        let expired_lane_full = expired_leases.len() == AUTH_SWEEP_LEASE_LANE_BATCH_SIZE;

        let mut remaining =
            AUTH_SWEEP_BATCH_SIZE - pending.len() - current.len() - expired_leases.len();
        if remaining > 0 && expired_lane_full {
            if let Some(epoch) = proposed.expired_lease.clone() {
                let cutoff = epoch
                    .cutoff
                    .expect("expired credential lease epoch has a cutoff");
                let mut extra = self
                    .store
                    .credentials_with_expired_mint_leases(
                        cutoff,
                        epoch.after.as_ref(),
                        epoch.through_sequence,
                        remaining,
                    )
                    .await?;
                proposed.expired_lease =
                    finish_sweep_page(&extra, remaining, &epoch, expired_credential_lease_position);
                remaining -= extra.len();
                expired_leases.append(&mut extra);
            }
        }
        if remaining > 0 && pending_lane_full {
            if let Some(epoch) = proposed.pending.clone() {
                let mut extra = self
                    .store
                    .pending_credentials_due_for_reissue(
                        epoch.after.as_ref(),
                        epoch.through_sequence,
                        remaining,
                    )
                    .await?;
                proposed.pending =
                    finish_sweep_page(&extra, remaining, &epoch, pending_reissue_position);
                remaining -= extra.len();
                pending.append(&mut extra);
            }
        }
        if remaining > 0 && current_lane_full {
            if let Some(epoch) = proposed.current.clone() {
                let cutoff = epoch.cutoff.expect("current reissue epoch has a cutoff");
                let mut extra = self
                    .store
                    .current_credentials_due_for_reissue(
                        cutoff,
                        epoch.after.as_ref(),
                        epoch.through_sequence,
                        remaining,
                    )
                    .await?;
                proposed.current =
                    finish_sweep_page(&extra, remaining, &epoch, current_reissue_position);
                current.append(&mut extra);
            }
        }

        // Publish the proposed epochs only after every lane read needed for
        // this batch succeeds. A later read failure therefore returns no work
        // and advances no earlier lane.
        *epochs = proposed;
        drop(epochs);

        Ok(credential_reissue_batch(expired_leases, pending, current))
    }

    /// Select recurring reissue work while prepaying a bounded attempt for
    /// every returned row before any epoch cursor is published.
    pub(super) async fn sessions_due_for_reissue_with_budget(
        &self,
        lifetime: std::time::Duration,
        budget: &AuthOperationBudget,
    ) -> Result<SweepBatch<SessionId>, StoreError> {
        const ROW_OPERATIONS: usize = 1 + AUTH_REISSUE_ATTEMPT_BUDGET;
        const PAGE_LIMIT: usize = (AUTH_SWEEP_THREE_LANE_BATCH_SIZE - 1) / ROW_OPERATIONS;

        let now = SystemTime::now();
        let issued_before = now.checked_sub(lifetime).unwrap_or(SystemTime::UNIX_EPOCH);
        let mut epochs = self.reissue_sweep_epochs.lock().await;
        let mut proposed = epochs.clone();
        let needs_high_water = proposed.pending.is_none()
            || proposed.current.is_none()
            || proposed.expired_lease.is_none();
        let reserved = usize::from(needs_high_water) + 3 + 3 * PAGE_LIMIT * ROW_OPERATIONS;
        if PAGE_LIMIT == 0 || !budget.take(reserved) {
            return Ok(SweepBatch {
                work: Vec::new(),
                first_error: None,
            });
        }

        if needs_high_water {
            let through_sequence = self.store.auth_sweep_high_water().await?;
            proposed.pending.get_or_insert(SweepEpoch {
                cutoff: None,
                through_sequence,
                after: None,
            });
            proposed.current.get_or_insert(SweepEpoch {
                cutoff: Some(issued_before),
                through_sequence,
                after: None,
            });
            proposed.expired_lease.get_or_insert(SweepEpoch {
                cutoff: Some(now),
                through_sequence,
                after: None,
            });
        }

        let pending_epoch = proposed
            .pending
            .clone()
            .expect("pending reissue epoch was initialized above");
        let pending = self
            .store
            .pending_credentials_due_for_reissue(
                pending_epoch.after.as_ref(),
                pending_epoch.through_sequence,
                PAGE_LIMIT,
            )
            .await?;
        if pending.len() > PAGE_LIMIT {
            return Err(StoreError::Malformed(
                "pending credential store exceeded its requested page limit".into(),
            ));
        }
        proposed.pending = finish_sweep_page(
            &pending,
            PAGE_LIMIT,
            &pending_epoch,
            pending_reissue_position,
        );

        let current_epoch = proposed
            .current
            .clone()
            .expect("current reissue epoch was initialized above");
        let current_cutoff = current_epoch
            .cutoff
            .expect("current reissue epoch has a cutoff");
        let current = self
            .store
            .current_credentials_due_for_reissue(
                current_cutoff,
                current_epoch.after.as_ref(),
                current_epoch.through_sequence,
                PAGE_LIMIT,
            )
            .await?;
        if current.len() > PAGE_LIMIT {
            return Err(StoreError::Malformed(
                "current credential store exceeded its requested page limit".into(),
            ));
        }
        proposed.current = finish_sweep_page(
            &current,
            PAGE_LIMIT,
            &current_epoch,
            current_reissue_position,
        );

        let lease_epoch = proposed
            .expired_lease
            .clone()
            .expect("expired credential lease epoch was initialized above");
        let lease_cutoff = lease_epoch
            .cutoff
            .expect("expired credential lease epoch has a cutoff");
        let expired_leases = self
            .store
            .credentials_with_expired_mint_leases(
                lease_cutoff,
                lease_epoch.after.as_ref(),
                lease_epoch.through_sequence,
                PAGE_LIMIT,
            )
            .await?;
        if expired_leases.len() > PAGE_LIMIT {
            return Err(StoreError::Malformed(
                "expired credential lease store exceeded its requested page limit".into(),
            ));
        }
        proposed.expired_lease = finish_sweep_page(
            &expired_leases,
            PAGE_LIMIT,
            &lease_epoch,
            expired_credential_lease_position,
        );

        let returned = pending.len() + current.len() + expired_leases.len();
        let used = usize::from(needs_high_water) + 3 + returned * ROW_OPERATIONS;
        budget.refund(reserved - used);
        *epochs = proposed;
        drop(epochs);
        Ok(credential_reissue_batch(expired_leases, pending, current))
    }

    /// Mint, or resume delivering, a fresh credential for one session.
    ///
    /// The provider request belongs to a detached task. Cancelling the caller
    /// only stops it waiting: a successful mint is still recorded in `pending`
    /// for the next sweep, rather than becoming an unowned no-expiry key. The
    /// current credential remains issued until [`Self::confirm_reissue`] says
    /// the runtime accepted this value.
    #[cfg(test)]
    pub async fn reissue(
        &self,
        session: &layer2_core::SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential(session, subject, services, true)
            .await
    }

    pub(super) async fn reissue_with_budget(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
        budget: AuthOperationBudget,
    ) -> Result<Option<String>, StoreError> {
        self.mint_session_credential_with_budget(session, subject, services, true, budget)
            .await
    }

    /// Promote a pending replacement after its runtime accepted the new value.
    ///
    /// A generation compare-and-swap moves the old current value to durable
    /// revoke debt in the same transition that promotes the replacement.
    #[cfg(test)]
    pub async fn confirm_reissue(
        &self,
        session: &layer2_core::SessionId,
    ) -> Result<bool, StoreError> {
        self.promote_pending_credential(session).await
    }

    pub(super) async fn confirm_reissue_with_budget(
        &self,
        session: &SessionId,
        budget: &AuthOperationBudget,
    ) -> Result<bool, StoreError> {
        self.promote_pending_credential_with_budget(session, budget)
            .await
    }

    /// Take back credentials queued on an earlier pass or release.
    #[cfg(test)]
    pub async fn revoke_retired_credentials(&self) -> Result<usize, StoreError> {
        let budget = AuthOperationBudget::standalone(AUTH_SWEEP_BATCH_SIZE);
        self.revoke_credential_debt(&budget).await
    }

    pub(super) async fn revoke_retired_credentials_with_budget(
        &self,
        budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        self.revoke_credential_debt(budget).await
    }

    pub(super) async fn reclaim_orphaned_credentials_with_budgets(
        &self,
        service: &Arc<layer2_core::SessionService>,
        active_budget: &AuthOperationBudget,
        provider_budget: &AuthOperationBudget,
    ) -> Result<usize, StoreError> {
        self.reclaim_unrecorded_credentials_with_budgets(service, active_budget, provider_budget)
            .await
    }

    /// Replace a subject fact with revision-CAS, retrying a concurrent writer.
    async fn replace_subject_fact<F>(
        &self,
        subject: &SubjectId,
        mut update: F,
    ) -> Result<(), StoreError>
    where
        F: FnMut(&mut SubjectAuthState) -> Result<(), StoreError>,
    {
        loop {
            let current = self.store.subject(subject).await?;
            let mut next = current.clone().unwrap_or_else(|| SubjectAuthState {
                subject: subject.clone(),
                incarnation: random_token(),
                revision: 0,
                token_generation: 0,
                tokens: None,
                refresh_lease: None,
                authorization: Authorization::none(),
                authorization_check_after: SystemTime::UNIX_EPOCH,
                profile: profile::Profile::default(),
            });
            update(&mut next)?;
            let written = match current {
                Some(current) => {
                    next.revision = current.revision.checked_add(1).ok_or_else(|| {
                        StoreError::Malformed("subject auth revision exhausted".into())
                    })?;
                    self.store
                        .replace_subject(&current.incarnation, current.revision, next)
                        .await?
                }
                None => self.store.insert_subject(next).await?,
            };
            if written {
                return Ok(());
            }
        }
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
            state.token_generation = next_token_generation(state.token_generation)?;
            state.tokens = Some(tokens.clone());
            state.refresh_lease = None;
            state.authorization = authorization.clone();
            // The sweep ticker is globally aligned, not relative to sign-in.
            // Due-now guarantees this complete answer is checked on the next
            // tick rather than narrowly missing it and waiting two periods.
            state.authorization_check_after = SystemTime::now();
            state.profile = profile.clone();
            Ok(())
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
            state.token_generation = next_token_generation(state.token_generation)?;
            state.tokens = Some(durable.clone());
            state.refresh_lease = None;
            Ok(())
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
    #[cfg(test)]
    pub async fn subjects_needing_refresh(
        &self,
        margin: std::time::Duration,
    ) -> Result<SweepBatch<DueSubjectRefresh>, StoreError> {
        let now = SystemTime::now();
        let refresh_at = now + margin;
        let mut epochs = self.refresh_sweep_epochs.lock().await;
        let mut proposed = epochs.clone();
        if proposed.unleased.is_none() || proposed.expired_lease.is_none() {
            let through_sequence = self.store.auth_sweep_high_water().await?;
            proposed.unleased.get_or_insert(SweepEpoch {
                cutoff: Some(refresh_at),
                through_sequence,
                after: None,
            });
            proposed.expired_lease.get_or_insert(SweepEpoch {
                cutoff: Some(now),
                through_sequence,
                after: None,
            });
        }

        let mut states = if let Some(epoch) = proposed.unleased.clone() {
            let cutoff = epoch.cutoff.expect("unleased refresh epoch has a cutoff");
            let states = self
                .store
                .unleased_subjects_due_for_refresh(
                    cutoff,
                    epoch.after.as_ref(),
                    epoch.through_sequence,
                    AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
                )
                .await?;
            proposed.unleased = finish_sweep_page(
                &states,
                AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
                &epoch,
                subject_refresh_position,
            );
            states
        } else {
            Vec::new()
        };
        let unleased_lane_full = states.len() == AUTH_SWEEP_TWO_LANE_BATCH_SIZE;

        let mut expired_leases = if let Some(epoch) = proposed.expired_lease.clone() {
            let cutoff = epoch
                .cutoff
                .expect("expired refresh lease epoch has a cutoff");
            let states = self
                .store
                .subjects_with_expired_refresh_leases(
                    cutoff,
                    epoch.after.as_ref(),
                    epoch.through_sequence,
                    AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
                )
                .await?;
            proposed.expired_lease = finish_sweep_page(
                &states,
                AUTH_SWEEP_TWO_LANE_BATCH_SIZE,
                &epoch,
                subject_refresh_position,
            );
            states
        } else {
            Vec::new()
        };
        let expired_lane_full = expired_leases.len() == AUTH_SWEEP_TWO_LANE_BATCH_SIZE;

        let remaining = AUTH_SWEEP_BATCH_SIZE - states.len() - expired_leases.len();
        if remaining > 0 && unleased_lane_full {
            if let Some(epoch) = proposed.unleased.clone() {
                let cutoff = epoch.cutoff.expect("unleased refresh epoch has a cutoff");
                let mut extra = self
                    .store
                    .unleased_subjects_due_for_refresh(
                        cutoff,
                        epoch.after.as_ref(),
                        epoch.through_sequence,
                        remaining,
                    )
                    .await?;
                proposed.unleased =
                    finish_sweep_page(&extra, remaining, &epoch, subject_refresh_position);
                states.append(&mut extra);
            }
        } else if remaining > 0 && expired_lane_full {
            if let Some(epoch) = proposed.expired_lease.clone() {
                let cutoff = epoch
                    .cutoff
                    .expect("expired refresh lease epoch has a cutoff");
                let mut extra = self
                    .store
                    .subjects_with_expired_refresh_leases(
                        cutoff,
                        epoch.after.as_ref(),
                        epoch.through_sequence,
                        remaining,
                    )
                    .await?;
                proposed.expired_lease =
                    finish_sweep_page(&extra, remaining, &epoch, subject_refresh_position);
                expired_leases.append(&mut extra);
            }
        }

        // Publish the proposed epochs only after every lane read needed for
        // this batch succeeds. A later read failure therefore returns no work
        // and advances no earlier lane.
        *epochs = proposed;
        drop(epochs);

        states.append(&mut expired_leases);
        Ok(subject_refresh_batch(states))
    }

    /// Select recurring refresh work while prepaying a bounded attempt for
    /// every returned row before either lane cursor is published.
    pub(super) async fn subjects_needing_refresh_with_budget(
        &self,
        margin: std::time::Duration,
        budget: &AuthOperationBudget,
    ) -> Result<SweepBatch<DueSubjectRefresh>, StoreError> {
        const ROW_OPERATIONS: usize = 1 + AUTH_REFRESH_ATTEMPT_BUDGET;
        const PAGE_LIMIT: usize = (AUTH_SWEEP_TWO_LANE_BATCH_SIZE - 1) / ROW_OPERATIONS;

        let now = SystemTime::now();
        let refresh_at = now + margin;
        let mut epochs = self.refresh_sweep_epochs.lock().await;
        let mut proposed = epochs.clone();
        let needs_high_water = proposed.unleased.is_none() || proposed.expired_lease.is_none();
        let reserved = usize::from(needs_high_water) + 2 + 2 * PAGE_LIMIT * ROW_OPERATIONS;
        if PAGE_LIMIT == 0 || !budget.take(reserved) {
            return Ok(SweepBatch {
                work: Vec::new(),
                first_error: None,
            });
        }

        if needs_high_water {
            let through_sequence = self.store.auth_sweep_high_water().await?;
            proposed.unleased.get_or_insert(SweepEpoch {
                cutoff: Some(refresh_at),
                through_sequence,
                after: None,
            });
            proposed.expired_lease.get_or_insert(SweepEpoch {
                cutoff: Some(now),
                through_sequence,
                after: None,
            });
        }

        let unleased_epoch = proposed
            .unleased
            .clone()
            .expect("unleased refresh epoch was initialized above");
        let unleased_cutoff = unleased_epoch
            .cutoff
            .expect("unleased refresh epoch has a cutoff");
        let mut states = self
            .store
            .unleased_subjects_due_for_refresh(
                unleased_cutoff,
                unleased_epoch.after.as_ref(),
                unleased_epoch.through_sequence,
                PAGE_LIMIT,
            )
            .await?;
        if states.len() > PAGE_LIMIT {
            return Err(StoreError::Malformed(
                "subject refresh store exceeded its requested page limit".into(),
            ));
        }
        proposed.unleased = finish_sweep_page(
            &states,
            PAGE_LIMIT,
            &unleased_epoch,
            subject_refresh_position,
        );

        let lease_epoch = proposed
            .expired_lease
            .clone()
            .expect("expired refresh lease epoch was initialized above");
        let lease_cutoff = lease_epoch
            .cutoff
            .expect("expired refresh lease epoch has a cutoff");
        let mut expired_leases = self
            .store
            .subjects_with_expired_refresh_leases(
                lease_cutoff,
                lease_epoch.after.as_ref(),
                lease_epoch.through_sequence,
                PAGE_LIMIT,
            )
            .await?;
        if expired_leases.len() > PAGE_LIMIT {
            return Err(StoreError::Malformed(
                "expired subject refresh lease store exceeded its requested page limit".into(),
            ));
        }
        proposed.expired_lease = finish_sweep_page(
            &expired_leases,
            PAGE_LIMIT,
            &lease_epoch,
            subject_refresh_position,
        );

        let returned = states.len() + expired_leases.len();
        let used = usize::from(needs_high_water) + 2 + returned * ROW_OPERATIONS;
        budget.refund(reserved - used);
        *epochs = proposed;
        drop(epochs);
        states.append(&mut expired_leases);
        Ok(subject_refresh_batch(states))
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

fn subject_refresh_batch(
    states: Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>,
) -> SweepBatch<DueSubjectRefresh> {
    let mut first_error = None;
    let mut work = Vec::new();
    for item in states {
        let state = match item.state {
            Ok(state) => state,
            Err(error) => {
                remember_sweep_item_error(&mut first_error, error, "read subject refresh item");
                continue;
            }
        };
        let Some(tokens) = state.tokens else {
            continue;
        };
        let Some(refresh) = tokens.refresh_token else {
            continue;
        };
        work.push(DueSubjectRefresh {
            subject: state.subject,
            refresh_token: refresh.into_inner(),
            incarnation: state.incarnation,
            token_generation: state.token_generation,
        });
    }
    SweepBatch { work, first_error }
}

fn credential_reissue_batch(
    expired_leases: Vec<AuthSweepItem<CredentialLeaseCursor, persistence::SessionCredentialState>>,
    pending: Vec<AuthSweepItem<CredentialMutationCursor, persistence::SessionCredentialState>>,
    current: Vec<AuthSweepItem<CredentialReissueCursor, persistence::SessionCredentialState>>,
) -> SweepBatch<SessionId> {
    let mut first_error = None;
    let mut work = Vec::new();
    for state in expired_leases
        .into_iter()
        .map(|item| item.state)
        .chain(pending.into_iter().map(|item| item.state))
        .chain(current.into_iter().map(|item| item.state))
    {
        let state = match state {
            Ok(state) => state,
            Err(error) => {
                remember_sweep_item_error(&mut first_error, error, "read credential reissue item");
                continue;
            }
        };
        if state.pending.is_some() || state.current.is_some() {
            work.push(state.session);
        }
    }
    SweepBatch { work, first_error }
}

fn finish_sweep_page<T, C: Clone>(
    states: &[T],
    limit: usize,
    epoch: &SweepEpoch<C>,
    position: fn(&T) -> Option<C>,
) -> Option<SweepEpoch<C>> {
    if states.len() < limit {
        return None;
    }
    let mut next = epoch.clone();
    next.after = states.last().and_then(position);
    next.after.as_ref()?;
    Some(next)
}

fn pending_reissue_position(
    item: &AuthSweepItem<CredentialMutationCursor, persistence::SessionCredentialState>,
) -> Option<CredentialMutationCursor> {
    Some(item.cursor.clone())
}

fn current_reissue_position(
    item: &AuthSweepItem<CredentialReissueCursor, persistence::SessionCredentialState>,
) -> Option<CredentialReissueCursor> {
    Some(item.cursor.clone())
}

fn expired_credential_lease_position(
    item: &AuthSweepItem<CredentialLeaseCursor, persistence::SessionCredentialState>,
) -> Option<CredentialLeaseCursor> {
    Some(item.cursor.clone())
}

fn subject_refresh_position(
    item: &AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>,
) -> Option<SubjectRefreshCursor> {
    Some(item.cursor.clone())
}

fn next_token_generation(current: u64) -> Result<u64, StoreError> {
    current
        .checked_add(1)
        .ok_or_else(|| StoreError::Malformed("subject token generation exhausted".into()))
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
#[cfg(test)]
pub(crate) use delegation::empty_session_service;
mod identity;
mod middleware;
mod reconciliation;
#[cfg(test)]
pub(crate) use reconciliation::AuthorizationCheckOutcome;
#[cfg(test)]
mod reconciliation_tests;
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
