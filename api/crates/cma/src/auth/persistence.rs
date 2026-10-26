//! Durable authority required by browser and bearer authentication.

use std::time::SystemTime;

#[cfg(test)]
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};

use layer2_core::{Authorization, StoreError, SubjectId};
use sha2::{Digest as _, Sha256};
#[cfg(test)]
use tokio::sync::Mutex;
use zeroize::Zeroize;

use super::profile::Profile;

mod api_bearer;
pub use api_bearer::ApiBearerCustody;

/// A secret value that redacts diagnostics and clears owned bytes on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SecretValue(<redacted {} bytes>)", self.0.len())
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// One-way identifier for a secret presented by a browser.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretDigest([u8; 32]);

impl SecretDigest {
    pub fn identify(domain: &str, value: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(domain.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        Self(digest.finalize().into())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SecretDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretDigest(<one-way>)")
    }
}

#[derive(Debug, Clone)]
pub struct BrowserLogin {
    pub id: SecretDigest,
    pub subject: SubjectId,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct LoginAttempt {
    pub id: SecretDigest,
    pub browser_binding: SecretDigest,
    pub verifier: SecretValue,
    /// Validated application-relative path restored after sign-in.
    pub return_to: Option<String>,
    /// Server-created authority intent; never inferred from return_to.
    pub actor_link_intent: Option<super::actor_links::ActorLoginIntent>,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableSubjectTokens {
    pub access_token: SecretValue,
    pub refresh_token: Option<SecretValue>,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRefreshLease {
    pub holder: String,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct SubjectAuthState {
    pub subject: SubjectId,
    pub incarnation: String,
    pub revision: u64,
    pub token_generation: u64,
    pub refresh_lease: Option<SubjectRefreshLease>,
    pub authorization: Authorization,
    pub profile: Profile,
    pub tokens: Option<DurableSubjectTokens>,
}

impl SubjectAuthState {
    pub fn new(
        subject: SubjectId,
        authorization: Authorization,
        profile: Profile,
        tokens: Option<DurableSubjectTokens>,
    ) -> Self {
        Self {
            subject,
            incarnation: super::token::random_token(),
            revision: 1,
            token_generation: 1,
            refresh_lease: None,
            authorization,
            profile,
            tokens,
        }
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        if self.incarnation.trim().is_empty()
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.token_generation == 0
            || self.token_generation > self.revision
            || self.refresh_lease.as_ref().is_some_and(|lease| {
                lease.holder.trim().is_empty()
                    || lease.expires_at <= SystemTime::UNIX_EPOCH
                    || self.tokens.is_none()
            })
        {
            return Err(StoreError::Malformed(
                "subject auth fencing is invalid".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_replacement(&self, expected_revision: u64) -> Result<(), StoreError> {
        self.validate()?;
        if self.revision != next_auth_counter(expected_revision)? {
            return Err(StoreError::Malformed(
                "subject auth revision must advance once".into(),
            ));
        }
        Ok(())
    }
}

pub(super) fn next_auth_counter(value: u64) -> Result<u64, StoreError> {
    value
        .checked_add(1)
        .filter(|next| *next <= i64::MAX as u64)
        .ok_or_else(|| StoreError::Malformed("subject auth counter exhausted".into()))
}

#[async_trait::async_trait]
pub trait AuthStore: Send + Sync {
    async fn insert_proxy_grant(
        &self,
        _grant: super::nyxid_proxy::ProxyGrant,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "identity proxy grants are not configured".into(),
        ))
    }
    async fn proxy_grant(
        &self,
        _id: &[u8; 32],
    ) -> Result<Option<super::nyxid_proxy::ProxyGrant>, StoreError> {
        Err(StoreError::Unavailable(
            "identity proxy grants are not configured".into(),
        ))
    }
    async fn due_actor_wakeups(
        &self,
        _due_before: u64,
        _limit: usize,
    ) -> Result<Vec<super::actor_links::ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor wakeups are not configured".into(),
        ))
    }
    async fn due_actor_links(
        &self,
        _renewed_before: u64,
        _limit: usize,
    ) -> Result<Vec<super::actor_links::ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn actor_link_for_actor(
        &self,
        _actor_key: &str,
    ) -> Result<Option<super::actor_links::ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn actor_link_by_id(
        &self,
        _link_id: &str,
    ) -> Result<Option<super::actor_links::ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn replace_actor_link(
        &self,
        _expected_revision: Option<u64>,
        _link: super::actor_links::ActorLinkRecord,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    /// Immutable, independently held API credential; never replaces a login.
    async fn insert_api_bearer(&self, custody: ApiBearerCustody) -> Result<bool, StoreError>;
    async fn api_bearer(&self, id: &SecretDigest) -> Result<Option<ApiBearerCustody>, StoreError>;
    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError>;
    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
        browser_binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError>;
    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError>;
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError>;
    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError>;
    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError>;
    /// Successful login alone may replace an exact unfenced current-schema row.
    async fn replace_unfenced_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError>;
    async fn replace_subject(
        &self,
        expected_incarnation: &str,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError>;
    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError>;
}

/// In-memory implementation used by focused auth tests and trusted bring-up.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryAuthStore {
    unavailable: AtomicBool,
    attempts: Mutex<HashMap<SecretDigest, LoginAttempt>>,
    browsers: Mutex<HashMap<SecretDigest, BrowserLogin>>,
    subjects: Mutex<HashMap<SubjectId, SubjectAuthState>>,
    api_bearers: Mutex<HashMap<SecretDigest, ApiBearerCustody>>,
    actor_links: Mutex<HashMap<String, super::actor_links::ActorLinkRecord>>,
    proxy_grants: Mutex<HashMap<[u8; 32], super::nyxid_proxy::ProxyGrant>>,
}

#[cfg(test)]
impl MemoryAuthStore {
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
    }

    pub async fn put_subject(&self, state: SubjectAuthState) -> Result<(), StoreError> {
        self.available()?;
        state.validate()?;
        self.subjects
            .lock()
            .await
            .insert(state.subject.clone(), state);
        Ok(())
    }

    fn available(&self) -> Result<(), StoreError> {
        if self.unavailable.load(Ordering::SeqCst) {
            Err(StoreError::Unavailable(
                "authentication store is unavailable".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl AuthStore for MemoryAuthStore {
    async fn insert_proxy_grant(
        &self,
        grant: super::nyxid_proxy::ProxyGrant,
    ) -> Result<bool, StoreError> {
        self.available()?;
        grant.validate()?;
        let mut rows = self.proxy_grants.lock().await;
        if rows.contains_key(&grant.id) {
            return Ok(false);
        }
        rows.insert(grant.id, grant);
        Ok(true)
    }
    async fn proxy_grant(
        &self,
        id: &[u8; 32],
    ) -> Result<Option<super::nyxid_proxy::ProxyGrant>, StoreError> {
        self.available()?;
        Ok(self.proxy_grants.lock().await.get(id).cloned())
    }
    async fn due_actor_wakeups(
        &self,
        due_before: u64,
        limit: usize,
    ) -> Result<Vec<super::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        let rows = self.actor_links.lock().await;
        let mut due = rows
            .values()
            .filter(|row| {
                row.active
                    && !row.revoked
                    && row
                        .wakeup
                        .as_ref()
                        .and_then(|wakeup| wakeup.due_at())
                        .is_some_and(|at| at <= due_before)
            })
            .cloned()
            .collect::<Vec<_>>();
        due.sort_by_key(|row| row.wakeup.as_ref().and_then(|wakeup| wakeup.due_at()));
        due.truncate(limit.min(100));
        Ok(due)
    }
    async fn due_actor_links(
        &self,
        renewed_before: u64,
        limit: usize,
    ) -> Result<Vec<super::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        Ok(self
            .actor_links
            .lock()
            .await
            .values()
            .filter(|row| row.active && !row.revoked && row.renewed_at <= renewed_before)
            .take(limit)
            .cloned()
            .collect())
    }
    async fn actor_link_for_actor(
        &self,
        actor_key: &str,
    ) -> Result<Option<super::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        Ok(self.actor_links.lock().await.get(actor_key).cloned())
    }
    async fn actor_link_by_id(
        &self,
        link_id: &str,
    ) -> Result<Option<super::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        Ok(self
            .actor_links
            .lock()
            .await
            .values()
            .find(|row| row.link_id == link_id)
            .cloned())
    }
    async fn replace_actor_link(
        &self,
        expected_revision: Option<u64>,
        link: super::actor_links::ActorLinkRecord,
    ) -> Result<bool, StoreError> {
        self.available()?;
        link.validate()?;
        let mut rows = self.actor_links.lock().await;
        if rows.get(&link.actor_key).map(|row| row.revision) != expected_revision {
            return Ok(false);
        }
        rows.insert(link.actor_key.clone(), link);
        Ok(true)
    }
    async fn insert_api_bearer(&self, custody: ApiBearerCustody) -> Result<bool, StoreError> {
        self.available()?;
        custody.validate()?;
        let mut records = self.api_bearers.lock().await;
        if records.contains_key(&custody.id) {
            return Ok(false);
        }
        records.insert(custody.id.clone(), custody);
        Ok(true)
    }

    async fn api_bearer(&self, id: &SecretDigest) -> Result<Option<ApiBearerCustody>, StoreError> {
        self.available()?;
        Ok(self.api_bearers.lock().await.get(id).cloned())
    }
    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError> {
        self.available()?;
        self.attempts
            .lock()
            .await
            .insert(attempt.id.clone(), attempt);
        Ok(())
    }

    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
        browser_binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        self.available()?;
        let mut attempts = self.attempts.lock().await;
        if attempts
            .get(id)
            .is_some_and(|attempt| &attempt.browser_binding == browser_binding)
        {
            Ok(attempts.remove(id))
        } else {
            Ok(None)
        }
    }

    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError> {
        self.available()?;
        self.browsers.lock().await.insert(login.id.clone(), login);
        Ok(())
    }

    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        self.available()?;
        Ok(self.browsers.lock().await.get(id).cloned())
    }

    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.available()?;
        self.browsers.lock().await.remove(id);
        Ok(())
    }

    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        self.available()?;
        state.validate()?;
        let mut subjects = self.subjects.lock().await;
        if subjects.contains_key(&state.subject) {
            return Ok(false);
        }
        subjects.insert(state.subject.clone(), state);
        Ok(true)
    }

    async fn replace_unfenced_subject(&self, _state: SubjectAuthState) -> Result<bool, StoreError> {
        self.available()?;
        Ok(false)
    }

    async fn replace_subject(
        &self,
        expected_incarnation: &str,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        self.available()?;
        state.validate_replacement(expected_revision)?;
        let mut subjects = self.subjects.lock().await;
        if subjects.get(&state.subject).is_none_or(|current| {
            current.incarnation != expected_incarnation || current.revision != expected_revision
        }) {
            return Ok(false);
        }
        subjects.insert(state.subject.clone(), state);
        Ok(true)
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.available()?;
        Ok(self.subjects.lock().await.get(subject).cloned())
    }
}

#[cfg(test)]
#[path = "persistence_tests.rs"]
mod tests;
