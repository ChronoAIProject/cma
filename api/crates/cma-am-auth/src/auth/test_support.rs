//! Authentication state fixtures shared with the composing binary.

use std::sync::Arc;

use super::persistence::{AuthStore, MemoryAuthStore};
#[cfg(feature = "test-support")]
use super::{cookie_authorization, service_authorization, ProviderMetadata};
use super::{AuthMode, AuthState, SignInRecords};
#[cfg(feature = "test-support")]
use tokio::sync::RwLock;

pub fn contract_auth(origin: &str) -> Arc<AuthState> {
    AuthState::in_memory(
        AuthMode::Provider {
            base_url: origin.into(),
            client_id: "contract-client".into(),
            client_secret: "contract-client-fixture".into(),
            public_url: "https://cma.example.test".into(),
        },
        None,
    )
}

pub use super::cookie_authorization::{
    VerificationKey, MAX_VERIFICATION_FLIGHTS, PROVIDER_VERIFICATION_TIMEOUT,
};

pub fn capture_desktop(
    auth: Arc<AuthState>,
    headers: &axum::http::HeaderMap,
    resolved: &super::RequestAuth,
) -> Result<Arc<dyn layer2_core::sandbox::DesktopAuthorization>, layer2_core::sandbox::DesktopError>
{
    super::desktop::capture(auth, headers, resolved)
}

pub use super::agent_keys::is_agent_key;
pub use super::cookie_authorization::{
    request_for_browser_cookie, request_for_cookie, request_with_timeout, verify_with_timeout,
};
pub use super::flow::code_challenge as authorization_code_challenge;
pub use super::refresh::refresh_if_needed;
#[cfg(feature = "test-support")]
pub use super::resource_request::request_descriptor;
pub use super::resource_request::HEADER as RESOURCE_REQUEST_HEADER;
pub use super::worker_credentials::matches_path as matches_worker_credentials_path;

pub mod identity {
    pub use super::super::identity::{request_for_bearer, resolve_subject, verify_subject};
}

impl AuthState {
    #[cfg(feature = "test-support")]
    pub fn fixture_active_receipts(&self) -> usize {
        self.identity_store.active_receipts()
    }

    #[cfg(feature = "test-support")]
    pub fn clear_fixture_identity_store(&self) {
        self.identity_store.clear();
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_token_epoch(&self, token: &str) -> Option<Arc<()>> {
        self.identity_store
            .token_epoch(&self.identity_store.key(token))
    }

    #[cfg(feature = "test-support")]
    pub fn invalidate_fixture_token(&self, token: &str) {
        self.identity_store.invalidate_token(token);
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_verification_slots(&self) -> &tokio::sync::Semaphore {
        &self.verification_slots
    }

    #[cfg(feature = "test-support")]
    pub fn with_fixture_grant_authority(
        mut self: Arc<Self>,
        authority: Arc<dyn service_authorization::GrantAuthority>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("test authority is configured before sharing")
            .service_authorization = Some(authority);
        self
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_grant_authority(&self) -> Option<&dyn service_authorization::GrantAuthority> {
        self.service_authorization.as_deref()
    }

    #[cfg(feature = "test-support")]
    pub fn set_fixture_standing(
        &mut self,
        standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) {
        self.standing = standing;
    }

    #[cfg(feature = "test-support")]
    pub fn enable_fixture_account_deletion(&mut self) {
        self.account_deletion_enabled = true;
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_cookie_verifications(&self) -> &cookie_authorization::VerificationFlights {
        &self.cookie_verifications
    }

    #[cfg(feature = "test-support")]
    pub fn enable_fixture_browser_session_join(&mut self) {
        self.browser_session_join_enabled = true;
    }

    #[cfg(feature = "test-support")]
    pub fn set_fixture_http(&mut self, http: reqwest::Client) {
        self.http = http;
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_mode(&self) -> &AuthMode {
        &self.mode
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_http(&self) -> &reqwest::Client {
        &self.http
    }

    #[cfg(feature = "test-support")]
    pub fn fixture_metadata(&self) -> &RwLock<Option<ProviderMetadata>> {
        &self.metadata
    }

    #[cfg(feature = "test-support")]
    pub fn set_fixture_store(&mut self, store: Arc<dyn AuthStore>) {
        self.store = store;
    }
    pub fn in_memory(mode: AuthMode, records: Option<SignInRecords>) -> Arc<Self> {
        Self::with_store(mode, records, Arc::new(MemoryAuthStore::default()))
    }

    pub fn with_store(
        mode: AuthMode,
        records: Option<SignInRecords>,
        store: Arc<dyn AuthStore>,
    ) -> Arc<Self> {
        let nyxid_public_url = match &mode {
            AuthMode::Provider { base_url, .. } => Some(base_url.clone()),
            AuthMode::Trusted { .. } => None,
        };
        Self::with_store_and_nyxid_public_url(mode, records, store, nyxid_public_url)
    }
}
