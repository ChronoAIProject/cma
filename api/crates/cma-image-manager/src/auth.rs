//! Image Manager binds the technical auth client to its own NyxID resource.
pub use cma_service_auth::{AuthError, Principal};
#[derive(Clone)]
pub struct NyxIdAuth(pub cma_service_auth::NyxIdAuth);
impl NyxIdAuth {
    pub fn new(
        issuer: &str,
        resource: &str,
        client_id: &str,
        secret: String,
    ) -> Result<Self, AuthError> {
        if resource
            != format!(
                "{}/api/v1/proxy/s/cma-image-manager",
                issuer.trim_end_matches('/')
            )
        {
            return Err(AuthError::Configuration);
        }
        cma_service_auth::NyxIdAuth::new(
            issuer,
            cma_service_auth::Service::ImageManager,
            client_id,
            secret,
        )
        .map(Self)
    }
    pub async fn authenticate(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<Principal, AuthError> {
        self.0.authenticate(headers).await
    }
}
