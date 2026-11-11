//! Credential composition for current Sandboxes and retained legacy cleanup.
use crate::{
    auth::{keys::KeyIssuer, AuthMode},
    config::Config,
};
use std::sync::Arc;

pub(super) fn provider_tools(
    config: &Config,
    stores: &crate::store::MongoStores,
) -> Result<(Arc<crate::store::MongoProviderToolSource>, axum::Router), String> {
    let source = crate::store::MongoProviderToolSource::with_relay(
        stores.sandbox_model.clone(),
        stores.sandbox_credentials.clone(),
        config.provider_tool_relay.clone(),
        &config.public_url,
        &config.auth_encryption_key,
    )?;
    let routes = crate::provider_tool_relay::router(source.clone());
    Ok((source, routes))
}

pub(super) fn agent_access_tools(
    config: &Config,
    stores: &crate::store::MongoStores,
) -> Result<Arc<crate::store::MongoAgentAccessToolSource>, String> {
    crate::store::MongoAgentAccessToolSource::with_retained_origins(
        stores.sandbox_model.clone(),
        &config.public_url,
        &config.agent_access_retained_origins,
        &config.auth_encryption_key,
    )
}

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
            let tokens =
                crate::sandbox::ServiceAccessOwnerTokens::new(auth, stores.sandbox_model.clone());
            ManagedSandboxCredentials::new(issuer, tokens, stores.sandbox_credentials.clone())
        }
        None => Arc::new(UnavailableSandboxCredentials),
    };
    crate::sandbox::legacy_credentials::LegacySandboxCredentials::new(
        ordinary,
        stores.sandbox_model.clone(),
    )
}

pub(super) fn authentication(
    config: &Config,
    stores: &crate::store::MongoStores,
    users: &Arc<layer2_core::UserDirectory>,
    activity: &Arc<layer2_core::ActivityService>,
    settings: Arc<layer2_core::PlatformSettingsService>,
) -> Result<Arc<crate::auth::AuthState>, String> {
    use crate::auth::{AuthState, SignInRecords};
    let auth = AuthState::with_store_and_nyxid_public_url(
        config.auth_mode.clone(),
        Some(SignInRecords {
            users: Arc::clone(users),
            activity: Arc::clone(activity),
        }),
        stores.auth.clone(),
        config.nyxid_public_url.clone(),
    )
    .with_platform_settings(settings)
    .with_browser_origins(&config.cors_origins)
    .with_agent_key_service(config.nyxid_service_id.clone())
    .with_oauth_api_resource(config.nyxid_oauth_api_resource.clone())
    .with_resource_request_enabled(config.features.nyxid_resource_request)
    .with_identity_proxy(
        config.features.nyxid_proxy,
        config.nyxid_proxy_issuer.clone(),
        config.nyxid_proxy_jwks_url.clone(),
        config.nyxid_proxy_audience.clone(),
    )?
    .with_binding_store(stores.sandbox_model.clone())
    .with_bot_father_username(
        config
            .bot_father
            .as_ref()
            .map(|father| father.username.clone()),
    )?;
    if !auth.requires_credential() {
        tracing::warn!(
            "running with a trusted fixed subject; this mode is for local development only"
        );
    }
    Ok(auth)
}

pub(super) fn agent_skills(
    config: &Config,
    credentials: Arc<dyn crate::sandbox::SandboxCredentialController>,
) -> Result<Arc<crate::sandbox::OrnnAgentSkillResolver>, String> {
    use crate::sandbox::OrnnAgentSkillResolver;

    match &config.auth_mode {
        AuthMode::Provider { .. } => {
            OrnnAgentSkillResolver::scoped(&config.runtime.platform_base_url, credentials)
        }
        AuthMode::Trusted { .. } => {
            OrnnAgentSkillResolver::anonymous(&config.runtime.platform_base_url)
        }
    }
}
