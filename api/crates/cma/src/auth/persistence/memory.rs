//! Test implementation with atomic pending-custody promotion and deletion.
use super::*;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
};
use tokio::sync::Mutex;

/// In-memory implementation used by focused auth tests and trusted bring-up.
#[derive(Default)]
pub struct MemoryAuthStore {
    unavailable: AtomicBool,
    attempts: Mutex<HashMap<SecretDigest, LoginAttempt>>,
    browsers: Mutex<HashMap<SecretDigest, BrowserLogin>>,
    pending_browsers: Mutex<HashMap<SecretDigest, SystemTime>>,
    subjects: Mutex<HashMap<SubjectId, SubjectAuthState>>,
    api_bearers: Mutex<HashMap<SecretDigest, ApiBearerCustody>>,
    actor_links: Mutex<HashMap<String, crate::auth::actor_links::ActorLinkRecord>>,
    proxy_grants: Mutex<HashMap<[u8; 32], crate::auth::nyxid_proxy::ProxyGrant>>,
}

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

#[async_trait::async_trait]
impl AuthStore for MemoryAuthStore {
    async fn insert_proxy_grant(
        &self,
        grant: crate::auth::nyxid_proxy::ProxyGrant,
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
    ) -> Result<Option<crate::auth::nyxid_proxy::ProxyGrant>, StoreError> {
        self.available()?;
        Ok(self.proxy_grants.lock().await.get(id).cloned())
    }
    async fn due_actor_wakeups(
        &self,
        due_before: u64,
        limit: usize,
    ) -> Result<Vec<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
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
    ) -> Result<Vec<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
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
    ) -> Result<Option<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        Ok(self.actor_links.lock().await.get(actor_key).cloned())
    }
    async fn actor_link_by_id(
        &self,
        link_id: &str,
    ) -> Result<Option<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
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
        link: crate::auth::actor_links::ActorLinkRecord,
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

    async fn reserve_browser_login(
        &self,
        id: SecretDigest,
        expires_at: SystemTime,
    ) -> Result<(), StoreError> {
        self.available()?;
        self.pending_browsers.lock().await.insert(id, expires_at);
        Ok(())
    }

    async fn complete_browser_login(&self, login: BrowserLogin) -> Result<bool, StoreError> {
        self.available()?;
        let mut pending = self.pending_browsers.lock().await;
        if pending
            .remove(&login.id)
            .is_none_or(|expiry| expiry <= SystemTime::now())
        {
            return Ok(false);
        }
        self.browsers.lock().await.insert(login.id.clone(), login);
        Ok(true)
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
        let mut pending = self.pending_browsers.lock().await;
        pending.remove(id);
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
