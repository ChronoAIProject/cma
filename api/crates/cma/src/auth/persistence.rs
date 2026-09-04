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
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct DurableSubjectTokens {
    pub access_token: SecretValue,
    pub refresh_token: Option<SecretValue>,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct SubjectAuthState {
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub profile: Profile,
    pub tokens: Option<DurableSubjectTokens>,
}

#[async_trait::async_trait]
pub trait AuthStore: Send + Sync {
    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError>;
    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
        browser_binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError>;
    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError>;
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError>;
    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError>;
    async fn put_subject(&self, state: SubjectAuthState) -> Result<(), StoreError>;
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
}

#[cfg(test)]
impl MemoryAuthStore {
    pub fn set_unavailable(&self, unavailable: bool) {
        self.unavailable.store(unavailable, Ordering::SeqCst);
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

    async fn put_subject(&self, state: SubjectAuthState) -> Result<(), StoreError> {
        self.available()?;
        self.subjects
            .lock()
            .await
            .insert(state.subject.clone(), state);
        Ok(())
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.available()?;
        Ok(self.subjects.lock().await.get(subject).cloned())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn secrets_are_one_way_and_redacted() {
        let first = SecretDigest::identify("cookie", "value");
        let second = SecretDigest::identify("other", "value");
        assert_ne!(first, second);
        assert!(!format!("{first:?}").contains("value"));
        assert!(!format!("{:?}", SecretValue::new("visible".into())).contains("visible"));
    }

    #[tokio::test]
    async fn login_attempts_are_binding_checked_and_single_use() {
        let store = MemoryAuthStore::default();
        let id = SecretDigest::identify("attempt", "state");
        let binding = SecretDigest::identify("binding", "browser");
        store
            .put_login_attempt(LoginAttempt {
                id: id.clone(),
                browser_binding: binding.clone(),
                verifier: SecretValue::new("verifier".into()),
                return_to: None,
                expires_at: SystemTime::now() + Duration::from_secs(60),
            })
            .await
            .unwrap();
        assert!(store
            .take_login_attempt(&id, &SecretDigest::identify("binding", "other"))
            .await
            .unwrap()
            .is_none());
        assert!(store
            .take_login_attempt(&id, &binding)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .take_login_attempt(&id, &binding)
            .await
            .unwrap()
            .is_none());
    }
}
