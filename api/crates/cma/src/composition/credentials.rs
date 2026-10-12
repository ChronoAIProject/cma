//! Credential composition for current Sandboxes and retained legacy cleanup.
use crate::{
    auth::{keys::KeyIssuer, AuthMode},
    config::Config,
};
use std::sync::Arc;
pub(super) fn key_issuer(config: &Config) -> Option<Arc<KeyIssuer>> {
    let AuthMode::Provider { base_url, .. } = &config.auth_mode else {
        return None;
    };
    Some(Arc::new(KeyIssuer::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("the rustls HTTP client builds"),
        base_url,
        &config.deployment_id,
    )))
}

pub(super) fn sandbox_credentials(
    config: &Config,
    auth: Arc<crate::auth::AuthState>,
    stores: &crate::store::MongoStores,
) -> Arc<dyn crate::sandbox::SandboxCredentialController> {
    use crate::sandbox::{
        ManagedSandboxCredentials, SandboxCredentialController, UnavailableSandboxCredentials,
    };
    let ordinary: Arc<dyn SandboxCredentialController> = match key_issuer(config) {
        Some(issuer) => {
            ManagedSandboxCredentials::new(issuer, auth, stores.sandbox_credentials.clone())
        }
        None => Arc::new(UnavailableSandboxCredentials),
    };
    crate::sandbox::legacy_credentials::LegacySandboxCredentials::new(
        ordinary,
        stores.sandbox_model.clone(),
    )
}
