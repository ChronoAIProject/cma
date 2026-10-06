//! Production ports for the public, sender-authorized Bot setup workflow.
mod authority;
mod authorization;
mod configuration;
mod connections;
mod continuation;
mod delegation;
mod events;
mod origin;
mod provisioning;
use authority::SetupAuthority;
pub(crate) use continuation::ActorContinuationWorker;
#[cfg(test)]
mod integration_tests;

use crate::{auth::AuthState, config::Config, store::MongoStores};
use layer2_core::{
    bot_setups::*,
    bots::{BotError, BotService},
    profiles::PrivateProfileCreationService,
    PlatformSettingsService,
};
use std::{sync::Arc, time::Duration};

pub(crate) struct SetupDependencies {
    authority: Arc<SetupAuthority>,
    stores: Arc<MongoStores>,
    settings: Arc<PlatformSettingsService>,
    bots: Arc<BotService>,
    profiles: Arc<PrivateProfileCreationService>,
    connections: crate::gateway::connections::ChannelConnections,
    required_services: Vec<String>,
}
impl std::ops::Deref for SetupDependencies {
    type Target = SetupAuthority;
    fn deref(&self) -> &Self::Target {
        &self.authority
    }
}
impl SetupDependencies {
    pub(crate) fn build(
        config: &Config,
        stores: Arc<MongoStores>,
        auth: Arc<AuthState>,
        settings: Arc<PlatformSettingsService>,
        bots: Arc<BotService>,
    ) -> Result<Arc<Self>, String> {
        let father = config
            .bot_father
            .as_ref()
            .ok_or("Bot Father is not configured")?;
        let gateway = config.gateway.as_ref().ok_or("Bot Father requires CMAEG")?;
        let crate::auth::AuthMode::Provider { .. } = &config.auth_mode else {
            return Err("Bot Father requires NyxID".into());
        };
        let policy = crate::profile_policy::profile_route_policy();
        let mut required_services = config
            .baseline_services
            .iter()
            .map(|s| s.as_str().to_owned())
            .collect::<Vec<_>>();
        for slug in ["chrono-llm-public", "cmaeg"] {
            if !required_services.iter().any(|s| s == slug) {
                required_services.push(slug.into());
            }
        }
        let dependencies = Arc::new(Self {
            profiles: PrivateProfileCreationService::new(
                stores.sandbox_profiles.clone(),
                policy.capacity_bounds,
                policy.public_value_scanner,
            ),
            stores: stores.clone(),
            settings,
            bots,
            authority: Arc::new(SetupAuthority {
                auth,
                store: stores.sandbox_model.clone(),
                custody: stores.sandbox_credentials.clone(),
                activity: stores.activity.clone(),
                contexts: stores.bot_setup_contexts.clone(),
                http: reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(3))
                    .timeout(Duration::from_secs(20))
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .user_agent("cma-bot-setup/1")
                    .build()
                    .map_err(|_| "setup HTTP client configuration failed")?,
                gateway: gateway.base_url.trim_end_matches('/').into(),
                binding: father.binding_id.clone(),
            }),
            connections: crate::gateway::connections::ChannelConnections::new(
                reqwest::Client::builder()
                    .connect_timeout(Duration::from_secs(3))
                    .timeout(Duration::from_secs(20))
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .build()
                    .map_err(|_| "connection HTTP client configuration failed")?,
                &gateway.base_url,
                &gateway.public_url,
            )?,
            required_services,
        });
        Ok(dependencies)
    }

    pub(crate) fn service(self: &Arc<Self>) -> Arc<BotSetupService> {
        BotSetupService::new(self.stores.bot_setups.clone(), self.clone())
    }
}
async fn request_json(request: reqwest::RequestBuilder) -> Result<serde_json::Value, BotError> {
    let mut response = request.send().await.map_err(|_| BotError::Unavailable)?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(BotError::Forbidden);
    }
    if !response.status().is_success() {
        return Err(BotError::Unavailable);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| BotError::Unavailable)? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(BotError::Unavailable);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| BotError::Unavailable)
}

#[async_trait::async_trait]
impl BotSetupDependencies for SetupDependencies {
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
        activity: Option<&serde_json::Value>,
    ) -> Result<SetupEvent, BotError> {
        self.verified_event(binding, credential, reference, activity)
            .await
    }
    async fn context(
        &self,
        binding: &str,
        credential: &str,
        conversation: &str,
        event: &str,
    ) -> Result<Option<serde_json::Value>, BotError> {
        self.retained_context(binding, credential, conversation, event)
            .await
    }
    async fn identity(&self, actor: &ExternalActor) -> Result<SetupIdentity, BotError> {
        self.link_identity(actor).await
    }
    async fn confirm_identity(
        &self,
        actor: &ExternalActor,
        proof: &str,
    ) -> Result<SetupIdentity, BotError> {
        self.confirm_link(actor, proof).await
    }
    async fn owners(
        &self,
        actor: &ExternalActor,
        grant: &str,
    ) -> Result<Vec<SetupOwnerChoice>, BotError> {
        self.owner_choices(actor, grant).await
    }
    async fn defaults(&self) -> Result<SetupConfiguration, BotError> {
        self.default_configuration().await
    }
    async fn validate_configuration(
        &self,
        configuration: &SetupConfiguration,
    ) -> Result<(), BotError> {
        self.validate(configuration).await
    }
    async fn services(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &layer2_core::resource_access::ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        self.service_choices(actor, grant, owner).await
    }
    async fn connect(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &layer2_core::resource_access::ResourceOwner,
        service: &str,
    ) -> Result<String, BotError> {
        self.auth
            .start_service_connection(grant, &authorization::actor(actor), owner, service)
            .await
            .map(|value| value.url)
    }
    async fn external_bots(
        &self,
        actor: &ExternalActor,
        grant: &str,
        owner: &layer2_core::resource_access::ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError> {
        self.channel_bots(actor, grant, owner).await
    }
    async fn connection_authority_stamp(&self, setup: &BotSetup) -> Result<String, BotError> {
        self.connection_stamp(setup).await
    }
    async fn channel_connection(
        &self,
        setup: &BotSetup,
    ) -> Result<SetupConnectionStatus, BotError> {
        self.resolve_connection(setup).await
    }
    async fn cancel_channel_connection(&self, setup: &BotSetup) -> Result<(), BotError> {
        self.cancel_connection(setup).await
    }
    async fn provision(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.create(setup).await
    }
    async fn result(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.current_result(setup).await
    }
}
