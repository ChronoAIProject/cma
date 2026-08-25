//! Durable authentication state and the protocol-neutral port that keeps it.

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::SystemTime;

use layer2_core::store::StoreError;
use layer2_core::{Authorization, SessionId, SubjectId};
use sha2::{Digest, Sha256};
#[cfg(test)]
use tokio::sync::Mutex;
use zeroize::Zeroize;

use super::profile::Profile;

/// A recoverable secret whose diagnostics never reveal its value.
pub struct SecretValue(String);

impl SecretValue {
    /// Take custody of plaintext secret material.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrow the plaintext only at the boundary that must present it.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Move the plaintext to another owner, clearing this allocation first.
    pub fn into_inner(mut self) -> String {
        std::mem::take(&mut self.0)
    }
}

impl Clone for SecretValue {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretValue(<redacted {} bytes>)", self.0.len())
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A one-way identifier for a high-entropy bearer value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretDigest([u8; 32]);

impl SecretDigest {
    /// Derive an identifier in a distinct record domain.
    pub fn identify(domain: &str, value: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(domain.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        Self(digest.finalize().into())
    }

    /// Rebuild an identifier read from durable storage.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Bytes used as the durable record key.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SecretDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretDigest(<sha256>)")
    }
}

/// A browser login session issued by this deployment.
#[derive(Debug, Clone)]
pub struct BrowserSession {
    /// Digest of the cookie value, never the bearer value itself.
    pub id: SecretDigest,
    /// Subject established by the completed provider flow.
    pub subject: SubjectId,
    /// Absolute application expiry. A TTL index only cleans up later.
    pub expires_at: SystemTime,
}

/// One in-flight authorization-code flow.
#[derive(Debug, Clone)]
pub struct LoginAttempt {
    /// Digest of the OAuth state returned through the browser.
    pub id: SecretDigest,
    /// PKCE verifier, encrypted by the concrete durable store.
    pub verifier: SecretValue,
    /// Absolute application expiry.
    pub expires_at: SystemTime,
}

/// Provider tokens retained for one subject.
#[derive(Debug, Clone)]
pub struct DurableSubjectTokens {
    /// Currently delegated access token.
    pub access_token: SecretValue,
    /// Token used for renewal, when the provider issued one.
    pub refresh_token: Option<SecretValue>,
    /// Absolute access-token expiry.
    pub expires_at: SystemTime,
}

/// Durable ownership of one provider refresh exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRefreshLease {
    /// Unique replica operation holding the exchange right.
    pub holder: String,
    /// When another worker may take over after process loss.
    pub expires_at: SystemTime,
}

/// Stable position in either time-ordered subject refresh work set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRefreshCursor {
    /// Token expiry or expired-lease deadline, depending on the work set.
    pub ready_at: SystemTime,
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Tie-breaker for subjects sharing the same deadline.
    pub subject: SubjectId,
}

/// Stable position in a token-owner scan ordered by durable mutation sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectOwnerCursor {
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Tie-breaker for defensive compatibility with pre-sequence records.
    pub subject: SubjectId,
}

/// Identity, grants, display material, and tokens stored as one subject fact.
#[derive(Debug, Clone)]
pub struct SubjectAuthState {
    /// Stable provider subject.
    pub subject: SubjectId,
    /// Random identity for this record lifecycle, renewed after TTL deletion.
    pub incarnation: String,
    /// Compare-and-swap revision.
    pub revision: u64,
    /// Generation of the stored token set, independent of lease-only revisions.
    pub token_generation: u64,
    /// Provider tokens, absent after delegation is withdrawn.
    pub tokens: Option<DurableSubjectTokens>,
    /// Fences refresh-token exchanges across replicas.
    pub refresh_lease: Option<SubjectRefreshLease>,
    /// Capabilities resolved from provider permissions.
    pub authorization: Authorization,
    /// Non-authoritative display material.
    pub profile: Profile,
}

/// One provider key whose full value can still be delivered to a runtime.
#[derive(Debug, Clone)]
pub struct DurableCredential {
    /// Provider identifier used for revocation.
    pub id: String,
    /// One-time full value, encrypted by the concrete store.
    pub value: SecretValue,
    /// Subject whose account owns the provider key.
    pub owner: SubjectId,
    /// Absolute issue time.
    pub issued_at: SystemTime,
    /// Provider-enforced expiry, once #524 supplies one.
    pub expires_at: Option<SystemTime>,
    /// Hash of the frozen service grant this key represents.
    pub scope_fingerprint: String,
}

/// Stable position in the time-ordered current-credential reissue work set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialReissueCursor {
    /// Issue time of the current credential.
    pub issued_at: SystemTime,
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Tie-breaker for sessions sharing the same issue time.
    pub session: SessionId,
}

/// Stable position in the expired credential-mint lease work set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialLeaseCursor {
    /// Deadline after which another worker may take over the mint.
    pub ready_at: SystemTime,
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Tie-breaker for sessions sharing the same deadline.
    pub session: SessionId,
}

/// Stable position in a credential scan ordered only by mutation sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialMutationCursor {
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Tie-breaker for defensive compatibility with pre-sequence records.
    pub session: SessionId,
}

/// A provider key no runtime should present and revocation still owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeDebt {
    /// Provider identifier used for revocation.
    pub id: String,
    /// Subject whose token authorizes the revoke.
    pub owner: SubjectId,
    /// When the debt first became durable.
    pub queued_at: SystemTime,
}

/// Stable position in the ordered revoke-debt work set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeDebtCursor {
    /// Mutation sequence of the credential record containing this debt.
    pub sweep_sequence: u64,
    /// Session containing the debt.
    pub session: SessionId,
    /// Provider identifier within that session's debt set.
    pub credential_id: String,
}

/// Stable opaque position in active credential-custody reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialReconciliationCursor {
    /// Mutation sequence that freezes this record version into one sweep epoch.
    pub sweep_sequence: u64,
    /// Last session custody record considered by the caller.
    pub session: SessionId,
}

/// One page member whose ordering key remains usable even when its body is corrupt.
pub struct AuthSweepItem<C, T> {
    /// Stable cursor extracted only from indexed, non-secret fields.
    pub cursor: C,
    /// Independently decoded body; one error must not suppress later page members.
    pub state: Result<T, StoreError>,
}

/// One bounded provider-revocation attempt.
#[derive(Debug, Clone)]
pub struct RevokeDebtWork {
    /// Session whose custody record carries this debt.
    pub session: SessionId,
    /// Provider key to revoke and then settle with generation CAS.
    pub debt: RevokeDebt,
}

/// Durable, generation-fenced ownership of one credential transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialLease {
    /// Unique worker attempting the transition.
    pub holder: String,
    /// Generation this worker is allowed to commit.
    pub fence: u64,
    /// When the intent was durably recorded.
    pub acquired_at: SystemTime,
    /// When another worker may take over.
    pub expires_at: SystemTime,
    /// Frozen grant the intended mint must carry.
    pub scope_fingerprint: String,
}

/// Durable credential custody for one session.
#[derive(Debug, Clone)]
pub struct SessionCredentialState {
    /// Session this state belongs to.
    pub session: SessionId,
    /// Compare-and-swap generation.
    pub generation: u64,
    /// Exact initiating bearer retained for no-issuer recovery.
    ///
    /// Absent means the session tracks its subject's renewable browser token.
    pub request_bound_credential: Option<SecretValue>,
    /// Credential a runtime is currently entitled to present.
    pub current: Option<DurableCredential>,
    /// Replacement minted but not yet confirmed delivered.
    pub pending: Option<DurableCredential>,
    /// Keys that must be revoked and retried until settled.
    pub revoke_debt: Vec<RevokeDebt>,
    /// Durable mint intent and generation fence.
    pub lease: Option<CredentialLease>,
    /// Terminal sessions reject any late mint or promotion.
    pub retired: bool,
}

impl SessionCredentialState {
    /// Whether this terminal fence has no credential work left to protect.
    pub(crate) fn is_settled_retired(&self) -> bool {
        self.retired
            && self.request_bound_credential.is_none()
            && self.current.is_none()
            && self.pending.is_none()
            && self.revoke_debt.is_empty()
            && self.lease.is_none()
    }
}

/// Every authentication fact that must survive a process or replica boundary.
#[async_trait::async_trait]
pub trait AuthStore: Send + Sync {
    /// Perform schema compatibility work within an explicit operation allowance.
    ///
    /// The returned count includes every store page, sequence allocation, and
    /// document write performed by the implementation. Implementations must
    /// never perform more than `max_operations`, including on failure.
    async fn maintain_auth_schema(&self, _max_operations: usize) -> Result<usize, StoreError> {
        Ok(0)
    }

    /// Highest durable mutation sequence allocated before a finite sweep starts.
    async fn auth_sweep_high_water(&self) -> Result<u64, StoreError>;

    /// Create or replace a browser session.
    async fn put_browser_session(&self, session: BrowserSession) -> Result<(), StoreError>;

    /// Read a browser session by the digest of its cookie.
    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<BrowserSession>, StoreError>;

    /// Revoke a browser session.
    async fn delete_browser_session(&self, id: &SecretDigest) -> Result<(), StoreError>;

    /// Record one authorization-code flow.
    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError>;

    /// Atomically consume one authorization-code flow.
    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError>;

    /// Read one subject's complete auth state.
    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError>;

    /// Insert only when no state exists for this subject.
    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError>;

    /// Replace only the expected subject revision.
    async fn replace_subject(
        &self,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError>;

    /// Unleased refreshable subjects due by `refresh_at`, in expiry order.
    async fn unleased_subjects_due_for_refresh(
        &self,
        refresh_at: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError>;

    /// Subjects whose abandoned refresh lease is due for takeover.
    async fn subjects_with_expired_refresh_leases(
        &self,
        now: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError>;

    /// Subjects with tokens used for provider orphan reconciliation, in stable-ID order.
    async fn subjects_with_tokens(
        &self,
        after: Option<&SubjectOwnerCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<SubjectOwnerCursor>, StoreError>;

    /// Read durable credential custody for one session.
    async fn credential(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionCredentialState>, StoreError>;

    /// Insert only when no credential state exists for this session.
    async fn insert_credential(&self, state: SessionCredentialState) -> Result<bool, StoreError>;

    /// Replace only the expected credential generation.
    async fn replace_credential(
        &self,
        expected_generation: u64,
        state: SessionCredentialState,
    ) -> Result<bool, StoreError>;

    /// Active current credentials old enough to replace, in issue-time order.
    async fn current_credentials_due_for_reissue(
        &self,
        issued_before: SystemTime,
        after: Option<&CredentialReissueCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialReissueCursor, SessionCredentialState>>, StoreError>;

    /// Active pending credentials awaiting delivery, in stable-ID order.
    async fn pending_credentials_due_for_reissue(
        &self,
        after: Option<&CredentialMutationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialMutationCursor, SessionCredentialState>>, StoreError>;

    /// Current credentials whose abandoned replacement lease can be resumed.
    async fn credentials_with_expired_mint_leases(
        &self,
        now: SystemTime,
        after: Option<&CredentialLeaseCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialLeaseCursor, SessionCredentialState>>, StoreError>;

    /// Credential states carrying revoke debt, up to `limit`.
    async fn revoke_debts(
        &self,
        after: Option<&RevokeDebtCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>>, StoreError>;

    /// Active custody records whose session claims must be reconciled.
    async fn active_credentials_for_reconciliation(
        &self,
        after: Option<&CredentialReconciliationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<CredentialReconciliationCursor>, StoreError>;
}

/// Process-local implementation used by unit tests.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryAuthStore {
    browser_sessions: Mutex<HashMap<SecretDigest, BrowserSession>>,
    login_attempts: Mutex<HashMap<SecretDigest, LoginAttempt>>,
    subjects: Mutex<HashMap<SubjectId, SubjectAuthState>>,
    subject_sweep_sequences: Mutex<HashMap<SubjectId, u64>>,
    credentials: Mutex<HashMap<SessionId, SessionCredentialState>>,
    credential_sweep_sequences: Mutex<HashMap<SessionId, u64>>,
    sweep_sequence: AtomicU64,
    unavailable: AtomicBool,
    scan_calls: AtomicUsize,
    fail_scan_call: AtomicUsize,
    failed_subject_read: Mutex<Option<SubjectId>>,
    failed_credential_read: Mutex<Option<SessionId>>,
    credential_replace_calls: AtomicUsize,
    fail_credential_replace_call: AtomicUsize,
    lose_credential_replace_response_call: AtomicUsize,
}

#[cfg(test)]
impl MemoryAuthStore {
    /// Inject or clear a store outage in failure-path tests.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    /// Fail one specific subsequent recurring-work read, numbered from one.
    pub fn fail_scan_on_call(&self, call: usize) {
        self.scan_calls.store(0, Ordering::SeqCst);
        self.fail_scan_call.store(call, Ordering::SeqCst);
    }

    /// Clear a recurring-work read failure injection.
    pub fn clear_scan_failure(&self) {
        self.fail_scan_call.store(0, Ordering::SeqCst);
    }

    /// Fail reads for one subject until cleared.
    pub async fn fail_subject_read(&self, subject: Option<SubjectId>) {
        *self.failed_subject_read.lock().await = subject;
    }

    /// Fail reads for one credential custody record until cleared.
    pub async fn fail_credential_read(&self, session: Option<SessionId>) {
        *self.failed_credential_read.lock().await = session;
    }

    /// Fail one selected credential replacement, counted after this call.
    pub fn fail_credential_replace_on_call(&self, call: usize) {
        self.credential_replace_calls.store(0, Ordering::SeqCst);
        self.lose_credential_replace_response_call
            .store(0, Ordering::SeqCst);
        self.fail_credential_replace_call
            .store(call, Ordering::SeqCst);
    }

    /// Apply one selected credential replacement but lose its acknowledgement.
    pub fn lose_credential_replace_response_on_call(&self, call: usize) {
        self.credential_replace_calls.store(0, Ordering::SeqCst);
        self.fail_credential_replace_call.store(0, Ordering::SeqCst);
        self.lose_credential_replace_response_call
            .store(call, Ordering::SeqCst);
    }

    /// Stop injecting credential replacement failures.
    pub fn clear_credential_replace_failure(&self) {
        self.fail_credential_replace_call.store(0, Ordering::SeqCst);
        self.lose_credential_replace_response_call
            .store(0, Ordering::SeqCst);
    }

    fn next_sweep_sequence(&self) -> u64 {
        self.sweep_sequence.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn available(&self) -> Result<(), StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("injected auth store outage".into()));
        }
        Ok(())
    }

    fn scan_available(&self) -> Result<(), StoreError> {
        self.available()?;
        let call = self.scan_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_scan_call.load(Ordering::SeqCst) == call {
            return Err(StoreError::Unavailable(
                "injected recurring-work scan failure".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
fn next_revision(current: u64, next: u64, record: &str) -> Result<(), StoreError> {
    if current.checked_add(1) != Some(next) {
        return Err(StoreError::Malformed(format!(
            "{record} revision must advance exactly once: {current} -> {next}"
        )));
    }
    Ok(())
}

#[cfg(test)]
fn subject_refresh_cursor_cmp(
    left: &SubjectRefreshCursor,
    right: &SubjectRefreshCursor,
) -> std::cmp::Ordering {
    left.ready_at
        .cmp(&right.ready_at)
        .then_with(|| left.sweep_sequence.cmp(&right.sweep_sequence))
        .then_with(|| left.subject.as_str().cmp(right.subject.as_str()))
}

#[cfg(test)]
fn subject_owner_cursor_cmp(
    left: &SubjectOwnerCursor,
    right: &SubjectOwnerCursor,
) -> std::cmp::Ordering {
    left.sweep_sequence
        .cmp(&right.sweep_sequence)
        .then_with(|| left.subject.as_str().cmp(right.subject.as_str()))
}

#[cfg(test)]
fn credential_reissue_cursor_cmp(
    left: &CredentialReissueCursor,
    right: &CredentialReissueCursor,
) -> std::cmp::Ordering {
    left.issued_at
        .cmp(&right.issued_at)
        .then_with(|| left.sweep_sequence.cmp(&right.sweep_sequence))
        .then_with(|| left.session.as_str().cmp(right.session.as_str()))
}

#[cfg(test)]
fn credential_lease_cursor_cmp(
    left: &CredentialLeaseCursor,
    right: &CredentialLeaseCursor,
) -> std::cmp::Ordering {
    left.ready_at
        .cmp(&right.ready_at)
        .then_with(|| left.sweep_sequence.cmp(&right.sweep_sequence))
        .then_with(|| left.session.as_str().cmp(right.session.as_str()))
}

#[cfg(test)]
fn credential_mutation_cursor_cmp(
    left: &CredentialMutationCursor,
    right: &CredentialMutationCursor,
) -> std::cmp::Ordering {
    left.sweep_sequence
        .cmp(&right.sweep_sequence)
        .then_with(|| left.session.as_str().cmp(right.session.as_str()))
}

#[cfg(test)]
fn reconciliation_cursor_cmp(
    left: &CredentialReconciliationCursor,
    right: &CredentialReconciliationCursor,
) -> std::cmp::Ordering {
    left.sweep_sequence
        .cmp(&right.sweep_sequence)
        .then_with(|| left.session.as_str().cmp(right.session.as_str()))
}

#[cfg(test)]
fn revoke_debt_cursor_cmp(left: &RevokeDebtCursor, right: &RevokeDebtCursor) -> std::cmp::Ordering {
    left.sweep_sequence
        .cmp(&right.sweep_sequence)
        .then_with(|| left.session.as_str().cmp(right.session.as_str()))
        .then_with(|| left.credential_id.cmp(&right.credential_id))
}

#[cfg(test)]
#[async_trait::async_trait]
impl AuthStore for MemoryAuthStore {
    async fn auth_sweep_high_water(&self) -> Result<u64, StoreError> {
        self.scan_available()?;
        Ok(self.sweep_sequence.load(Ordering::SeqCst))
    }

    async fn put_browser_session(&self, session: BrowserSession) -> Result<(), StoreError> {
        self.available()?;
        self.browser_sessions
            .lock()
            .await
            .insert(session.id.clone(), session);
        Ok(())
    }

    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<BrowserSession>, StoreError> {
        self.available()?;
        Ok(self.browser_sessions.lock().await.get(id).cloned())
    }

    async fn delete_browser_session(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.available()?;
        self.browser_sessions.lock().await.remove(id);
        Ok(())
    }

    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError> {
        self.available()?;
        self.login_attempts
            .lock()
            .await
            .insert(attempt.id.clone(), attempt);
        Ok(())
    }

    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        self.available()?;
        Ok(self.login_attempts.lock().await.remove(id))
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.available()?;
        if self.failed_subject_read.lock().await.as_ref() == Some(subject) {
            return Err(StoreError::Unavailable(
                "injected subject read failure".into(),
            ));
        }
        Ok(self.subjects.lock().await.get(subject).cloned())
    }

    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        self.available()?;
        if state.revision != 0 {
            return Err(StoreError::Malformed(
                "new subject auth state must start at revision zero".into(),
            ));
        }
        let mut subjects = self.subjects.lock().await;
        if subjects.contains_key(&state.subject) {
            return Ok(false);
        }
        let subject = state.subject.clone();
        subjects.insert(subject.clone(), state);
        self.subject_sweep_sequences
            .lock()
            .await
            .insert(subject, self.next_sweep_sequence());
        Ok(true)
    }

    async fn replace_subject(
        &self,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        self.available()?;
        next_revision(expected_revision, state.revision, "subject auth")?;
        let mut subjects = self.subjects.lock().await;
        let Some(current) = subjects.get(&state.subject) else {
            return Ok(false);
        };
        if current.revision != expected_revision {
            return Ok(false);
        }
        let subject = state.subject.clone();
        subjects.insert(subject.clone(), state);
        self.subject_sweep_sequences
            .lock()
            .await
            .insert(subject, self.next_sweep_sequence());
        Ok(true)
    }

    async fn unleased_subjects_due_for_refresh(
        &self,
        refresh_at: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        self.scan_available()?;
        let failed = self.failed_subject_read.lock().await.clone();
        let subjects = self.subjects.lock().await;
        let sequences = self.subject_sweep_sequences.lock().await;
        let mut page: Vec<_> = subjects
            .values()
            .filter_map(|state| {
                let sequence = *sequences.get(&state.subject)?;
                let tokens = state.tokens.as_ref()?;
                let cursor = SubjectRefreshCursor {
                    ready_at: tokens.expires_at,
                    sweep_sequence: sequence,
                    subject: state.subject.clone(),
                };
                (sequence <= through_sequence
                    && state.refresh_lease.is_none()
                    && tokens.expires_at <= refresh_at
                    && tokens.refresh_token.is_some()
                    && after.is_none_or(|after| subject_refresh_cursor_cmp(&cursor, after).is_gt()))
                .then(|| AuthSweepItem {
                    cursor,
                    state: if failed.as_ref() == Some(&state.subject) {
                        Err(StoreError::Unavailable(
                            "injected subject read failure".into(),
                        ))
                    } else {
                        Ok(state.clone())
                    },
                })
            })
            .collect();
        page.sort_by(|left, right| subject_refresh_cursor_cmp(&left.cursor, &right.cursor));
        page.truncate(limit);
        Ok(page)
    }

    async fn subjects_with_expired_refresh_leases(
        &self,
        now: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        self.scan_available()?;
        let failed = self.failed_subject_read.lock().await.clone();
        let subjects = self.subjects.lock().await;
        let sequences = self.subject_sweep_sequences.lock().await;
        let mut page: Vec<_> = subjects
            .values()
            .filter_map(|state| {
                let sequence = *sequences.get(&state.subject)?;
                let lease = state.refresh_lease.as_ref()?;
                let cursor = SubjectRefreshCursor {
                    ready_at: lease.expires_at,
                    sweep_sequence: sequence,
                    subject: state.subject.clone(),
                };
                (sequence <= through_sequence
                    && lease.expires_at <= now
                    && state
                        .tokens
                        .as_ref()
                        .is_some_and(|tokens| tokens.refresh_token.is_some())
                    && after.is_none_or(|after| subject_refresh_cursor_cmp(&cursor, after).is_gt()))
                .then(|| AuthSweepItem {
                    cursor,
                    state: if failed.as_ref() == Some(&state.subject) {
                        Err(StoreError::Unavailable(
                            "injected subject read failure".into(),
                        ))
                    } else {
                        Ok(state.clone())
                    },
                })
            })
            .collect();
        page.sort_by(|left, right| subject_refresh_cursor_cmp(&left.cursor, &right.cursor));
        page.truncate(limit);
        Ok(page)
    }

    async fn subjects_with_tokens(
        &self,
        after: Option<&SubjectOwnerCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<SubjectOwnerCursor>, StoreError> {
        self.scan_available()?;
        let subjects = self.subjects.lock().await;
        let sequences = self.subject_sweep_sequences.lock().await;
        let mut page: Vec<_> = subjects
            .values()
            .filter_map(|state| {
                let cursor = SubjectOwnerCursor {
                    sweep_sequence: *sequences.get(&state.subject)?,
                    subject: state.subject.clone(),
                };
                (state.tokens.is_some()
                    && cursor.sweep_sequence <= through_sequence
                    && after.is_none_or(|after| subject_owner_cursor_cmp(&cursor, after).is_gt()))
                .then_some(cursor)
            })
            .collect();
        page.sort_by(subject_owner_cursor_cmp);
        page.truncate(limit);
        Ok(page)
    }

    async fn credential(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionCredentialState>, StoreError> {
        self.available()?;
        if self.failed_credential_read.lock().await.as_ref() == Some(session) {
            return Err(StoreError::Unavailable(
                "injected credential read failure".into(),
            ));
        }
        Ok(self.credentials.lock().await.get(session).cloned())
    }

    async fn insert_credential(&self, state: SessionCredentialState) -> Result<bool, StoreError> {
        self.available()?;
        if state.generation != 0 {
            return Err(StoreError::Malformed(
                "new credential state must start at generation zero".into(),
            ));
        }
        let mut credentials = self.credentials.lock().await;
        if credentials.contains_key(&state.session) {
            return Ok(false);
        }
        let session = state.session.clone();
        credentials.insert(session.clone(), state);
        self.credential_sweep_sequences
            .lock()
            .await
            .insert(session, self.next_sweep_sequence());
        Ok(true)
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        state: SessionCredentialState,
    ) -> Result<bool, StoreError> {
        self.available()?;
        let call = self.credential_replace_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if self.fail_credential_replace_call.load(Ordering::SeqCst) == call {
            return Err(StoreError::Unavailable(
                "injected credential replacement failure".into(),
            ));
        }
        next_revision(expected_generation, state.generation, "credential")?;
        let mut credentials = self.credentials.lock().await;
        let Some(current) = credentials.get(&state.session) else {
            return Ok(false);
        };
        if current.generation != expected_generation {
            return Ok(false);
        }
        let session = state.session.clone();
        credentials.insert(session.clone(), state);
        self.credential_sweep_sequences
            .lock()
            .await
            .insert(session, self.next_sweep_sequence());
        if self
            .lose_credential_replace_response_call
            .load(Ordering::SeqCst)
            == call
        {
            return Err(StoreError::Unavailable(
                "injected credential replacement response loss".into(),
            ));
        }
        Ok(true)
    }

    async fn current_credentials_due_for_reissue(
        &self,
        issued_before: SystemTime,
        after: Option<&CredentialReissueCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialReissueCursor, SessionCredentialState>>, StoreError>
    {
        self.scan_available()?;
        let failed = self.failed_credential_read.lock().await.clone();
        let credentials = self.credentials.lock().await;
        let sequences = self.credential_sweep_sequences.lock().await;
        let mut page: Vec<_> = credentials
            .values()
            .filter_map(|state| {
                let sequence = *sequences.get(&state.session)?;
                let current = state.current.as_ref()?;
                let cursor = CredentialReissueCursor {
                    issued_at: current.issued_at,
                    sweep_sequence: sequence,
                    session: state.session.clone(),
                };
                (!state.retired
                    && state.lease.is_none()
                    && state.pending.is_none()
                    && current.issued_at <= issued_before
                    && sequence <= through_sequence
                    && after
                        .is_none_or(|after| credential_reissue_cursor_cmp(&cursor, after).is_gt()))
                .then(|| AuthSweepItem {
                    cursor,
                    state: if failed.as_ref() == Some(&state.session) {
                        Err(StoreError::Unavailable(
                            "injected credential read failure".into(),
                        ))
                    } else {
                        Ok(state.clone())
                    },
                })
            })
            .collect();
        page.sort_by(|left, right| credential_reissue_cursor_cmp(&left.cursor, &right.cursor));
        page.truncate(limit);
        Ok(page)
    }

    async fn pending_credentials_due_for_reissue(
        &self,
        after: Option<&CredentialMutationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialMutationCursor, SessionCredentialState>>, StoreError>
    {
        self.scan_available()?;
        let failed = self.failed_credential_read.lock().await.clone();
        let credentials = self.credentials.lock().await;
        let sequences = self.credential_sweep_sequences.lock().await;
        let mut page: Vec<_> = credentials
            .values()
            .filter_map(|state| {
                let cursor = CredentialMutationCursor {
                    sweep_sequence: *sequences.get(&state.session)?,
                    session: state.session.clone(),
                };
                (!state.retired
                    && state.lease.is_none()
                    && state.pending.is_some()
                    && cursor.sweep_sequence <= through_sequence
                    && after
                        .is_none_or(|after| credential_mutation_cursor_cmp(&cursor, after).is_gt()))
                .then(|| AuthSweepItem {
                    cursor,
                    state: if failed.as_ref() == Some(&state.session) {
                        Err(StoreError::Unavailable(
                            "injected credential read failure".into(),
                        ))
                    } else {
                        Ok(state.clone())
                    },
                })
            })
            .collect();
        page.sort_by(|left, right| credential_mutation_cursor_cmp(&left.cursor, &right.cursor));
        page.truncate(limit);
        Ok(page)
    }

    async fn credentials_with_expired_mint_leases(
        &self,
        now: SystemTime,
        after: Option<&CredentialLeaseCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialLeaseCursor, SessionCredentialState>>, StoreError> {
        self.scan_available()?;
        let failed = self.failed_credential_read.lock().await.clone();
        let credentials = self.credentials.lock().await;
        let sequences = self.credential_sweep_sequences.lock().await;
        let mut page: Vec<_> = credentials
            .values()
            .filter_map(|state| {
                let sequence = *sequences.get(&state.session)?;
                let lease = state.lease.as_ref()?;
                let cursor = CredentialLeaseCursor {
                    ready_at: lease.expires_at,
                    sweep_sequence: sequence,
                    session: state.session.clone(),
                };
                (!state.retired
                    && state.pending.is_none()
                    && state.current.is_some()
                    && lease.expires_at <= now
                    && sequence <= through_sequence
                    && after
                        .is_none_or(|after| credential_lease_cursor_cmp(&cursor, after).is_gt()))
                .then(|| AuthSweepItem {
                    cursor,
                    state: if failed.as_ref() == Some(&state.session) {
                        Err(StoreError::Unavailable(
                            "injected credential read failure".into(),
                        ))
                    } else {
                        Ok(state.clone())
                    },
                })
            })
            .collect();
        page.sort_by(|left, right| credential_lease_cursor_cmp(&left.cursor, &right.cursor));
        page.truncate(limit);
        Ok(page)
    }

    async fn revoke_debts(
        &self,
        after: Option<&RevokeDebtCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>>, StoreError> {
        self.scan_available()?;
        let credentials = self.credentials.lock().await;
        let sequences = self.credential_sweep_sequences.lock().await;
        let mut debts: Vec<_> = credentials
            .values()
            .flat_map(|state| {
                let sequence = sequences.get(&state.session).copied().unwrap_or_default();
                state.revoke_debt.iter().cloned().map(move |debt| {
                    let cursor = RevokeDebtCursor {
                        sweep_sequence: sequence,
                        session: state.session.clone(),
                        credential_id: debt.id.clone(),
                    };
                    AuthSweepItem {
                        cursor,
                        state: Ok(RevokeDebtWork {
                            session: state.session.clone(),
                            debt,
                        }),
                    }
                })
            })
            .filter(|item| {
                item.cursor.sweep_sequence <= through_sequence
                    && after.is_none_or(|after| revoke_debt_cursor_cmp(&item.cursor, after).is_gt())
            })
            .collect();
        debts.sort_by(|left, right| revoke_debt_cursor_cmp(&left.cursor, &right.cursor));
        debts.truncate(limit);
        Ok(debts)
    }

    async fn active_credentials_for_reconciliation(
        &self,
        after: Option<&CredentialReconciliationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<CredentialReconciliationCursor>, StoreError> {
        self.scan_available()?;
        let credentials = self.credentials.lock().await;
        let sequences = self.credential_sweep_sequences.lock().await;
        let mut page: Vec<_> = credentials
            .values()
            .filter_map(|state| {
                let cursor = CredentialReconciliationCursor {
                    sweep_sequence: *sequences.get(&state.session)?,
                    session: state.session.clone(),
                };
                (!state.retired
                    && cursor.sweep_sequence <= through_sequence
                    && after.is_none_or(|after| reconciliation_cursor_cmp(&cursor, after).is_gt()))
                .then_some(cursor)
            })
            .collect();
        page.sort_by(reconciliation_cursor_cmp);
        page.truncate(limit);
        Ok(page)
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
