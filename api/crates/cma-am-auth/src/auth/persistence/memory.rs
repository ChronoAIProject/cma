//! Test implementation with atomic pending-custody promotion and deletion.
use super::*;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::Mutex;

/// In-memory implementation used by focused auth tests and trusted bring-up.
#[derive(Default, Clone)]
pub struct MemoryAuthStore {
    unavailable: Arc<AtomicBool>,
    attempts: Arc<Mutex<HashMap<SecretDigest, LoginAttempt>>>,
    browsers: Arc<Mutex<BrowserCustody>>,
    subjects: Arc<Mutex<HashMap<SubjectId, SubjectAuthState>>>,
    api_bearers: Arc<Mutex<HashMap<SecretDigest, ApiBearerCustody>>>,
    actor_links: Arc<Mutex<HashMap<String, crate::auth::actor_links::ActorLinkRecord>>>,
    proxy_grants: Arc<Mutex<HashMap<[u8; 32], crate::auth::nyxid_proxy::ProxyGrant>>>,
    #[cfg(any(test, feature = "test-support"))]
    refresh_completion: Option<RefreshCompletionGate>,
}

#[cfg(any(test, feature = "test-support"))]
#[derive(Clone)]
struct RefreshCompletionGate {
    generation: u64,
    entered: Arc<tokio::sync::Semaphore>,
    release: Arc<tokio::sync::Semaphore>,
}

#[derive(Default)]
struct BrowserCustody {
    sessions: HashMap<SecretDigest, (BrowserLogin, Option<SecretDigest>)>,
    bindings: HashMap<SecretDigest, Reservation>,
}
struct Reservation {
    expires_at: SystemTime,
    linked_session: Option<SecretDigest>,
    revoked: bool,
    explicit_started_at: Option<SystemTime>,
    preference_revoked: bool,
}

impl MemoryAuthStore {
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_refresh_completion_gate(
        mut self,
        generation: u64,
        entered: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
    ) -> Self {
        self.refresh_completion = Some(RefreshCompletionGate {
            generation,
            entered,
            release,
        });
        self
    }

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

    async fn revoke_custody(
        &self,
        id: &SecretDigest,
        binding_cookie: bool,
        suppress: bool,
    ) -> Result<(), StoreError> {
        self.available()?;
        let mut rows = self.browsers.lock().await;
        let binding = if let Some((_, binding)) = rows.sessions.remove(id) {
            binding
        } else {
            (binding_cookie || rows.bindings.contains_key(id)).then(|| id.clone())
        };
        if let Some(binding) = binding {
            let previous = rows.bindings.remove(&binding);
            if let Some(linked) = previous.as_ref().and_then(|row| row.linked_session.clone()) {
                rows.sessions.remove(&linked);
            }
            rows.bindings.insert(
                binding,
                Reservation {
                    expires_at: (SystemTime::now() + crate::auth::BROWSER_BINDING_LIFETIME).max(
                        previous
                            .as_ref()
                            .map_or(SystemTime::UNIX_EPOCH, |row| row.expires_at),
                    ),
                    linked_session: None,
                    revoked: true,
                    explicit_started_at: previous.as_ref().and_then(|row| row.explicit_started_at),
                    preference_revoked: suppress
                        || previous.as_ref().is_some_and(|row| row.preference_revoked),
                },
            );
        }
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
        after_actor_key: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.available()?;
        let mut due = self
            .actor_links
            .lock()
            .await
            .values()
            .filter(|row| {
                row.active
                    && !row.revoked
                    && row.renewed_at <= renewed_before
                    && after_actor_key.is_none_or(|key| row.actor_key.as_str() > key)
            })
            .cloned()
            .collect::<Vec<_>>();
        due.sort_by(|left, right| left.actor_key.cmp(&right.actor_key));
        due.truncate(limit.clamp(1, 100));
        Ok(due)
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
    ) -> Result<bool, StoreError> {
        self.available()?;
        let mut rows = self.browsers.lock().await;
        if rows.bindings.contains_key(&id) || rows.sessions.contains_key(&id) {
            return Ok(false);
        }
        rows.bindings.insert(
            id,
            Reservation {
                expires_at,
                linked_session: None,
                revoked: false,
                explicit_started_at: None,
                preference_revoked: false,
            },
        );
        Ok(true)
    }
    async fn renew_browser_login(
        &self,
        binding: &SecretDigest,
        expires_at: SystemTime,
    ) -> Result<bool, StoreError> {
        self.available()?;
        let mut rows = self.browsers.lock().await;
        let Some(row) = rows.bindings.get_mut(binding) else {
            return Ok(false);
        };
        if row.revoked || row.linked_session.is_some() || row.expires_at <= SystemTime::now() {
            return Ok(false);
        }
        row.expires_at = row.expires_at.max(expires_at);
        Ok(true)
    }
    async fn complete_browser_login(
        &self,
        binding: &SecretDigest,
        login: BrowserLogin,
        explicit_started_at: Option<SystemTime>,
    ) -> Result<bool, StoreError> {
        self.available()?;
        if binding == &login.id {
            return Err(StoreError::Malformed(
                "session must differ from browser binding".into(),
            ));
        }
        let mut rows = self.browsers.lock().await;
        if rows.sessions.contains_key(&login.id) {
            return Ok(false);
        }
        let Some(row) = rows.bindings.get_mut(binding) else {
            return Ok(false);
        };
        if row.revoked || row.linked_session.is_some() || row.expires_at <= SystemTime::now() {
            return Ok(false);
        }
        row.linked_session = Some(login.id.clone());
        row.expires_at = if explicit_started_at.is_some() {
            login
                .expires_at
                .max(SystemTime::now() + crate::auth::SIGN_IN_PREFERENCE_LIFETIME)
        } else {
            login.expires_at
        };
        row.explicit_started_at = explicit_started_at;
        rows.sessions
            .insert(login.id.clone(), (login, Some(binding.clone())));
        Ok(true)
    }
    async fn browser_sign_in_preference(
        &self,
        binding: &SecretDigest,
        started_at: SystemTime,
    ) -> Result<bool, StoreError> {
        self.available()?;
        Ok(self
            .browsers
            .lock()
            .await
            .bindings
            .get(binding)
            .is_some_and(|row| {
                !row.preference_revoked
                    && row.expires_at > SystemTime::now()
                    && row.explicit_started_at.is_some_and(|started| {
                        crate::auth::silent_preference::timestamp(started)
                            == crate::auth::silent_preference::timestamp(started_at)
                    })
            }))
    }
    async fn logout_browser_custody(
        &self,
        id: &SecretDigest,
        binding_cookie: bool,
        suppress: bool,
    ) -> Result<(), StoreError> {
        self.revoke_custody(id, binding_cookie, suppress).await
    }
    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError> {
        self.available()?;
        self.browsers
            .lock()
            .await
            .sessions
            .insert(login.id.clone(), (login, None));
        Ok(())
    }
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        self.available()?;
        Ok(self
            .browsers
            .lock()
            .await
            .sessions
            .get(id)
            .map(|(login, _)| login.clone()))
    }
    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        let Some(login) = self.browser_login(id).await? else {
            return Ok(None);
        };
        let state = if login.expires_at > SystemTime::now() {
            self.subject(&login.subject).await?
        } else {
            None
        };
        Ok(Some((login, state)))
    }
    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.revoke_custody(id, false, true).await
    }
    async fn revoke_browser_binding(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.revoke_custody(id, true, true).await
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
        #[cfg(any(test, feature = "test-support"))]
        if let Some(gate) = self.refresh_completion.as_ref().filter(|gate| {
            state.refresh_lease.is_none()
                && state.tokens.is_some()
                && state.token_generation == gate.generation
        }) {
            gate.entered.add_permits(1);
            gate.release.acquire().await.unwrap().forget();
        }
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
