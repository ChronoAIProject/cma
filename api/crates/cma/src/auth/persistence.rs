//! Durable authentication state and the protocol-neutral port that keeps it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use layer2_core::store::StoreError;
use layer2_core::{Authorization, SessionId, SubjectId};
use sha2::{Digest, Sha256};
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

/// Identity, grants, display material, and tokens stored as one subject fact.
#[derive(Debug, Clone)]
pub struct SubjectAuthState {
    /// Stable provider subject.
    pub subject: SubjectId,
    /// Compare-and-swap revision.
    pub revision: u64,
    /// Provider tokens, absent after delegation is withdrawn.
    pub tokens: Option<DurableSubjectTokens>,
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

/// Every authentication fact that must survive a process or replica boundary.
#[async_trait::async_trait]
pub trait AuthStore: Send + Sync {
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

    /// Subjects whose recorded tokens may authorize provider work.
    async fn subjects(&self) -> Result<Vec<SubjectAuthState>, StoreError>;

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

    /// All credential states, used by bounded renewal and revoke sweeps.
    async fn credentials(&self) -> Result<Vec<SessionCredentialState>, StoreError>;
}

/// Process-local implementation used by unit tests.
#[derive(Default)]
pub struct MemoryAuthStore {
    browser_sessions: Mutex<HashMap<SecretDigest, BrowserSession>>,
    login_attempts: Mutex<HashMap<SecretDigest, LoginAttempt>>,
    subjects: Mutex<HashMap<SubjectId, SubjectAuthState>>,
    credentials: Mutex<HashMap<SessionId, SessionCredentialState>>,
    unavailable: AtomicBool,
}

impl MemoryAuthStore {
    /// Inject or clear a store outage in failure-path tests.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    fn available(&self) -> Result<(), StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StoreError::Unavailable("injected auth store outage".into()));
        }
        Ok(())
    }
}

fn next_revision(current: u64, next: u64, record: &str) -> Result<(), StoreError> {
    if next != current.saturating_add(1) {
        return Err(StoreError::Malformed(format!(
            "{record} revision must advance exactly once: {current} -> {next}"
        )));
    }
    Ok(())
}

#[async_trait::async_trait]
impl AuthStore for MemoryAuthStore {
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
        subjects.insert(state.subject.clone(), state);
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
        subjects.insert(state.subject.clone(), state);
        Ok(true)
    }

    async fn subjects(&self) -> Result<Vec<SubjectAuthState>, StoreError> {
        self.available()?;
        Ok(self.subjects.lock().await.values().cloned().collect())
    }

    async fn credential(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionCredentialState>, StoreError> {
        self.available()?;
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
        credentials.insert(state.session.clone(), state);
        Ok(true)
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        state: SessionCredentialState,
    ) -> Result<bool, StoreError> {
        self.available()?;
        next_revision(expected_generation, state.generation, "credential")?;
        let mut credentials = self.credentials.lock().await;
        let Some(current) = credentials.get(&state.session) else {
            return Ok(false);
        };
        if current.generation != expected_generation {
            return Ok(false);
        }
        credentials.insert(state.session.clone(), state);
        Ok(true)
    }

    async fn credentials(&self) -> Result<Vec<SessionCredentialState>, StoreError> {
        self.available()?;
        Ok(self.credentials.lock().await.values().cloned().collect())
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
