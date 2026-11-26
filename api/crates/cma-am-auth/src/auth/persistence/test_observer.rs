//! Test-only store boundary for deterministic wait and identity-race injection.
use super::*;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
pub struct ReadGate {
    pub reached: tokio::sync::Notify,
    pub release: tokio::sync::Notify,
}

pub struct ObservedAuthStore {
    pub inner: Arc<dyn AuthStore>,
    pub wait: Duration,
    pub wrong_subject: bool,
    pub reads: Mutex<Vec<(&'static str, &'static str)>>,
    pub held_read: Mutex<Option<(&'static str, Arc<ReadGate>)>>,
}
impl ObservedAuthStore {
    pub fn new(inner: Arc<dyn AuthStore>, wait: Duration) -> Self {
        Self {
            inner,
            wait,
            wrong_subject: false,
            reads: Mutex::new(Vec::new()),
            held_read: Mutex::new(None),
        }
    }
    async fn observe(&self, operation: &'static str) {
        self.reads.lock().unwrap().push((
            operation,
            layer2_core::diagnostic_context::DiagnosticContext::current().phase,
        ));
        let gate = {
            let mut held = self.held_read.lock().unwrap();
            if held.as_ref().is_some_and(|(name, _)| *name == operation) {
                held.take().map(|(_, gate)| gate)
            } else {
                None
            }
        };
        if let Some(gate) = gate {
            gate.reached.notify_one();
            gate.release.notified().await;
        }
        if !self.wait.is_zero() {
            tokio::time::sleep(self.wait).await;
        }
    }
}
#[async_trait::async_trait]
impl AuthStore for ObservedAuthStore {
    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError> {
        self.observe("join").await;
        let mut result = self.inner.browser_session(id).await?;
        if self.wrong_subject {
            if let Some((_, Some(state))) = &mut result {
                state.subject = SubjectId::new("crossed-fixture");
            }
        }
        Ok(result)
    }
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        self.observe("login").await;
        self.inner.browser_login(id).await
    }
    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.observe("subject").await;
        let mut result = self.inner.subject(subject).await?;
        if self.wrong_subject {
            if let Some(state) = &mut result {
                state.subject = SubjectId::new("crossed-fixture");
            }
        }
        Ok(result)
    }
    async fn insert_api_bearer(&self, custody: ApiBearerCustody) -> Result<bool, StoreError> {
        self.inner.insert_api_bearer(custody).await
    }
    async fn api_bearer(&self, id: &SecretDigest) -> Result<Option<ApiBearerCustody>, StoreError> {
        self.inner.api_bearer(id).await
    }
    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError> {
        self.inner.put_login_attempt(attempt).await
    }
    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
        binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        self.inner.take_login_attempt(id, binding).await
    }
    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError> {
        self.inner.put_browser_login(login).await
    }
    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.inner.delete_browser_login(id).await
    }
    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        self.inner.insert_subject(state).await
    }
    async fn replace_unfenced_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        self.inner.replace_unfenced_subject(state).await
    }
    async fn replace_subject(
        &self,
        incarnation: &str,
        revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        self.inner
            .replace_subject(incarnation, revision, state)
            .await
    }
}
