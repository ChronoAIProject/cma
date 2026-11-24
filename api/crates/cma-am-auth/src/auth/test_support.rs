//! Authentication state fixtures shared with the composing binary.

use std::sync::Arc;

use super::{AuthMode, AuthState};

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
