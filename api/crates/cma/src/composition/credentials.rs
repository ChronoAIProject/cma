//! Credential composition for current Sandboxes and retained legacy cleanup.
use crate::{
    auth::{keys::KeyIssuer, AuthMode},
    config::Config,
};
use std::sync::Arc;

pub(super) fn connection_broker(
    config: &Config,
    auth: Arc<crate::auth::AuthState>,
    subscriptions: Arc<crate::connections::CodexSubscriptions>,
) -> Result<Arc<crate::trigger::CmaServiceConnectionBroker>, String> {
    Ok(Arc::new(
        crate::trigger::CmaServiceConnectionBroker::from_auth_config_with_callback(
            &config.auth_mode,
            config.nyxid_public_url.as_deref(),
            Some(config.public_url.as_str()),
            auth,
        )
        .map_err(|error| format!("service connection broker is not usable: {error}"))?
        .with_subscriptions(subscriptions),
    ))
}

pub(super) struct Connections {
    pub broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
    pub browser: Arc<crate::connections::BrowserConnections>,
    pub prerequisite: Arc<crate::connections::ConnectionPrerequisite>,
}

pub(super) fn connections(
    config: &Config,
    auth: Arc<crate::auth::AuthState>,
    subscriptions: Arc<crate::connections::CodexSubscriptions>,
    stores: &Arc<crate::store::MongoStores>,
    policy: Arc<dyn layer2_core::sandbox::SandboxPolicySource>,
) -> Result<Connections, String> {
    let broker = connection_broker(config, auth.clone(), subscriptions.clone())?;
    let prerequisite =
        crate::connections::ConnectionPrerequisite::new(auth, broker.clone(), policy.clone())
            .with_exact_github(broker.clone());
    let browser = crate::connections::BrowserConnections::new(
        stores.clone(),
        policy,
        broker.clone(),
        subscriptions,
        (config.features.github_repository_preflight
            || config.features.github_connection_recovery
            || config.features.github_connection_approval)
            .then(|| broker.clone()),
        config.features.github_repository_preflight,
        config.features.github_connection_approval,
    );
    Ok(Connections {
        broker,
        browser,
        prerequisite,
    })
}

pub(super) fn provider_tools(
    config: &Config,
    stores: &crate::store::MongoStores,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Result<(Arc<crate::store::MongoProviderToolSource>, axum::Router), String> {
    let source = crate::store::MongoProviderToolSource::with_relay(
        stores.sandbox_model.clone(),
        stores.sandbox_credentials.clone(),
        config.provider_tool_relay.clone(),
        &config.public_url,
        &config.auth_encryption_key,
    )?
    .with_account_standing(standing);
    let routes = crate::provider_tool_relay::router(source.clone());
    Ok((source, routes))
}

pub(super) fn agent_access_tools(
    config: &Config,
    stores: &crate::store::MongoStores,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Result<Arc<crate::store::MongoAgentAccessToolSource>, String> {
    crate::store::MongoAgentAccessToolSource::with_retained_origins(
        stores.sandbox_model.clone(),
        &config.public_url,
        &config.agent_access_retained_origins,
        &config.auth_encryption_key,
    )
    .map(|source| source.with_account_standing(standing))
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
    broker: Arc<crate::trigger::CmaServiceConnectionBroker>,
) -> Arc<dyn crate::sandbox::SandboxCredentialController> {
    use crate::sandbox::{
        ManagedSandboxCredentials, SandboxCredentialController, UnavailableSandboxCredentials,
    };
    let ordinary: Arc<dyn SandboxCredentialController> = match key_issuer(config) {
        Some(issuer) => {
            let tokens =
                crate::sandbox::ServiceAccessOwnerTokens::new(auth, stores.sandbox_model.clone());
            ManagedSandboxCredentials::new(issuer, tokens, stores.sandbox_credentials.clone())
                .with_exact_connections(broker)
                .with_retirement_receipts(config.features.credential_retirement_receipts)
                .with_retirement_reauthorization(config.features.retirement_reauthorization)
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
    let auth = Authentication {
        mode: &config.auth_mode,
        nyxid_public_url: &config.nyxid_public_url,
        origins: &config.cors_origins,
        service_id: &config.nyxid_service_id,
        oauth_resource: &config.nyxid_oauth_api_resource,
        resource_request: config.features.nyxid_resource_request,
        silent_sign_in: config.features.silent_sign_in,
        browser_session_join: config.browser_session_join_enabled,
        identity_proxy: config.features.nyxid_proxy,
        proxy_require_request_proof: config.features.nyxid_proxy_require_request_proof,
        proxy_issuer: &config.nyxid_proxy_issuer,
        proxy_jwks: &config.nyxid_proxy_jwks_url,
        proxy_audience: &config.nyxid_proxy_audience,
    }
    .build(stores.auth.clone(), users, activity, settings)?
    .with_binding_store(stores.sandbox_model.clone())
    .with_platform_spend(stores.sandbox_model.clone())
    .with_service_authorization(
        config.features.service_authorization,
        config.service_authorization.as_ref(),
    )
    .with_proxy_replay_principal(config.features.nyxid_proxy_replay_principal)
    .with_account_deletion_enabled(config.account_deletion_enabled)
    .with_privacy_export_enabled(config.features.privacy_export_enabled)
    .with_binding_diagnostics(config.features.binding_diagnostics)
    .with_legal_terms(config.legal_terms.clone())
    .with_registration_capacity(stores.registration_capacity.clone())
    .with_bot_father_username(
        config
            .bot_father
            .as_ref()
            .map(|father| father.username.clone()),
    )?;
    Ok(auth)
}

pub(super) fn account_authentication(
    config: &crate::config::BackendFreeConfig,
    stores: &crate::store::mongo::MongoAccountStores,
    users: &Arc<layer2_core::UserDirectory>,
    activity: &Arc<layer2_core::ActivityService>,
    settings: Arc<layer2_core::PlatformSettingsService>,
) -> Result<Arc<crate::auth::AuthState>, String> {
    Ok(Authentication {
        mode: &config.authentication.auth_mode,
        nyxid_public_url: &config.authentication.nyxid_public_url,
        origins: &config.server.cors_origins,
        service_id: &config.nyxid_service_id,
        oauth_resource: &config.nyxid_oauth_api_resource,
        resource_request: config.nyxid_resource_request,
        silent_sign_in: config.silent_sign_in,
        browser_session_join: config.browser_session_join,
        identity_proxy: config.nyxid_proxy,
        proxy_require_request_proof: config.nyxid_proxy_require_request_proof,
        proxy_issuer: &config.nyxid_proxy_issuer,
        proxy_jwks: &config.nyxid_proxy_jwks_url,
        proxy_audience: &config.nyxid_proxy_audience,
    }
    .build(stores.auth.clone(), users, activity, settings)?
    .with_registration_policy(stores.registration_policy.clone())
    .with_registration_capacity(stores.registration.clone()))
}

struct Authentication<'a> {
    mode: &'a AuthMode,
    nyxid_public_url: &'a Option<String>,
    origins: &'a [axum::http::HeaderValue],
    service_id: &'a Option<String>,
    oauth_resource: &'a Option<String>,
    resource_request: bool,
    silent_sign_in: bool,
    browser_session_join: bool,
    identity_proxy: bool,
    proxy_require_request_proof: bool,
    proxy_issuer: &'a Option<String>,
    proxy_jwks: &'a Option<String>,
    proxy_audience: &'a Option<String>,
}

impl Authentication<'_> {
    fn build(
        self,
        store: Arc<crate::store::MongoAuthStore>,
        users: &Arc<layer2_core::UserDirectory>,
        activity: &Arc<layer2_core::ActivityService>,
        settings: Arc<layer2_core::PlatformSettingsService>,
    ) -> Result<Arc<crate::auth::AuthState>, String> {
        use crate::auth::{AuthState, SignInRecords};
        let auth = AuthState::with_store_and_nyxid_public_url(
            self.mode.clone(),
            Some(SignInRecords {
                users: Arc::clone(users),
                activity: Arc::clone(activity),
            }),
            store,
            self.nyxid_public_url.clone(),
        )
        .with_platform_settings(settings)
        .with_browser_origins(self.origins)
        .with_agent_key_service(self.service_id.clone())
        .with_oauth_api_resource(self.oauth_resource.clone())
        .with_resource_request_enabled(self.resource_request)
        .with_proxy_require_request_proof(self.proxy_require_request_proof)
        .with_silent_sign_in(self.silent_sign_in)
        .with_browser_session_join(self.browser_session_join)
        .with_identity_proxy(
            self.identity_proxy,
            self.proxy_issuer.clone(),
            self.proxy_jwks.clone(),
            self.proxy_audience.clone(),
        )?;
        if !auth.requires_credential() {
            tracing::warn!(
                "running with a trusted fixed subject; this mode is for local development only"
            );
        }
        Ok(auth)
    }
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
