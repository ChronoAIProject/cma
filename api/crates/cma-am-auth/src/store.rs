//! Durable storage interface used by Agent Manager authentication.

use std::{collections::BTreeSet, time::SystemTime};

use layer2_core::{ServiceId, StoreError, SubjectId};
use serde::{Deserialize, Serialize};

use super::{
    actor_links::{ActorLinkRecord, ActorLoginIntent},
    ApiBearerCustody, BrowserLogin, ProxyGrant, SecretDigest, SecretValue, SubjectAuthState,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryIntent {
    pub subject: SubjectId,
    pub issuer: String,
    pub required: BTreeSet<ServiceId>,
    pub requested: BTreeSet<ServiceId>,
    pub redirect_uri: String,
    pub return_to: String,
}

#[derive(Debug, Clone)]
pub struct LoginAttempt {
    pub id: SecretDigest,
    pub browser_binding: SecretDigest,
    pub verifier: SecretValue,
    /// Validated application-relative path restored after sign-in.
    pub return_to: Option<String>,
    /// Server-created authority intent; never inferred from return_to.
    pub actor_link_intent: Option<ActorLoginIntent>,
    pub service_authorization: Option<RecoveryIntent>,
    /// Asked the provider not to interact; a refusal returns to `return_to`.
    pub silent: bool,
    /// A silent-origin attempt has already escalated once to provider consent.
    pub consent_requested: bool,
    /// Absent on predecessor attempts, which are adopted before callback IO.
    pub custody_reserved: bool,
    /// Start of the explicit attempt, retained independently of response delivery.
    pub started_at: Option<SystemTime>,
    pub expires_at: SystemTime,
}

#[async_trait::async_trait]
pub trait AuthStore: Send + Sync {
    async fn insert_proxy_grant(&self, _grant: ProxyGrant) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "identity proxy grants are not configured".into(),
        ))
    }
    async fn proxy_grant(&self, _id: &[u8; 32]) -> Result<Option<ProxyGrant>, StoreError> {
        Err(StoreError::Unavailable(
            "identity proxy grants are not configured".into(),
        ))
    }
    async fn due_actor_wakeups(
        &self,
        _due_before: u64,
        _limit: usize,
    ) -> Result<Vec<ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor wakeups are not configured".into(),
        ))
    }
    async fn due_actor_links(
        &self,
        _renewed_before: u64,
        _after_actor_key: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn actor_link_for_actor(
        &self,
        _actor_key: &str,
    ) -> Result<Option<ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn actor_link_by_id(
        &self,
        _link_id: &str,
    ) -> Result<Option<ActorLinkRecord>, StoreError> {
        Err(StoreError::Unavailable(
            "actor links are not configured".into(),
        ))
    }
    async fn replace_actor_link(
        &self,
        _expected_revision: Option<u64>,
        _link: ActorLinkRecord,
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
    /// Reserve custody before the initiating-browser cookie is delivered. No authority yet.
    async fn reserve_browser_login(
        &self,
        _id: SecretDigest,
        _expires_at: SystemTime,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "browser login reservation is not configured".into(),
        ))
    }
    /// Atomically promote an unexpired reservation; never recreate one removed by logout.
    async fn complete_browser_login(
        &self,
        _binding: &SecretDigest,
        _login: BrowserLogin,
        _explicit_started_at: Option<SystemTime>,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "browser login fencing is not configured".into(),
        ))
    }
    async fn renew_browser_login(
        &self,
        _binding: &SecretDigest,
        _expires_at: SystemTime,
    ) -> Result<bool, StoreError> {
        Err(StoreError::Unavailable(
            "browser login renewal is not configured".into(),
        ))
    }
    async fn revoke_browser_binding(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.delete_browser_login(id).await
    }
    async fn browser_sign_in_preference(
        &self,
        _binding: &SecretDigest,
        _started_at: SystemTime,
    ) -> Result<bool, StoreError> {
        Ok(false)
    }
    async fn logout_browser_custody(
        &self,
        id: &SecretDigest,
        binding_cookie: bool,
        _suppress_preference: bool,
    ) -> Result<(), StoreError> {
        if binding_cookie {
            self.revoke_browser_binding(id).await
        } else {
            self.delete_browser_login(id).await
        }
    }
    #[cfg(feature = "test-support")]
    async fn put_browser_login(&self, _login: BrowserLogin) -> Result<(), StoreError> {
        Err(StoreError::Unavailable(
            "browser login fixture is unavailable".into(),
        ))
    }
    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError>;
    /// Read browser custody and its subject in one store operation. Expired logins
    /// need no subject decoding: the caller must revoke them before authorizing.
    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<(BrowserLogin, Option<SubjectAuthState>)>, StoreError>;
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
