//! Production ports for the public, sender-authorized Bot setup workflow.
mod authorization;
mod configuration;
mod events;
mod provisioning;

use crate::{auth::AuthState, config::Config, store::MongoStores};
use layer2_core::{
    bot_setups::*,
    bots::{BotError, BotService},
    profiles::PrivateProfileCreationService,
    PlatformSettingsService,
};
use std::{sync::Arc, time::Duration};

pub(crate) struct SetupDependencies {
    auth: Arc<AuthState>,
    stores: Arc<MongoStores>,
    settings: Arc<PlatformSettingsService>,
    bots: Arc<BotService>,
    profiles: Arc<PrivateProfileCreationService>,
    http: reqwest::Client,
    gateway: String,
    nyxid: String,
    nyxid_public: String,
    binding: String,
    required_services: Vec<String>,
}
impl SetupDependencies {
    pub(crate) fn build(
        config: &Config,
        stores: Arc<MongoStores>,
        auth: Arc<AuthState>,
        settings: Arc<PlatformSettingsService>,
        bots: Arc<BotService>,
    ) -> Result<Arc<BotSetupService>, String> {
        let father = config
            .bot_father
            .as_ref()
            .ok_or("Bot Father is not configured")?;
        let gateway = config.gateway.as_ref().ok_or("Bot Father requires CMAEG")?;
        let crate::auth::AuthMode::Provider { base_url, .. } = &config.auth_mode else {
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
            auth,
            stores: stores.clone(),
            settings,
            bots,
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(20))
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .user_agent("cma-bot-setup/1")
                .build()
                .map_err(|_| "setup HTTP client configuration failed")?,
            gateway: gateway.base_url.trim_end_matches('/').into(),
            nyxid: base_url.trim_end_matches('/').into(),
            nyxid_public: config
                .nyxid_public_url
                .as_deref()
                .unwrap_or(base_url)
                .trim_end_matches('/')
                .into(),
            binding: father.binding_id.clone(),
            required_services,
        });
        Ok(BotSetupService::new(
            stores.bot_setups.clone(),
            dependencies,
        ))
    }

    async fn json(&self, request: reqwest::RequestBuilder) -> Result<serde_json::Value, BotError> {
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
}

#[async_trait::async_trait]
impl BotSetupDependencies for SetupDependencies {
    async fn event(
        &self,
        binding: &str,
        credential: &str,
        reference: &str,
    ) -> Result<SetupEvent, BotError> {
        self.verified_event(binding, credential, reference).await
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
    async fn external_bot_registration_url(
        &self,
        owner: &layer2_core::resource_access::ResourceOwner,
    ) -> Result<String, BotError> {
        let mut url = reqwest::Url::parse(&format!(
            "{}/channel-bots/connect/telegram-new",
            self.nyxid_public
        ))
        .map_err(|_| BotError::Unavailable)?;
        if let layer2_core::resource_access::ResourceOwner::Organization { subject } = owner {
            url.query_pairs_mut()
                .append_pair("target_org_id", subject.as_str());
        }
        Ok(url.to_string())
    }
    async fn provision(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.create(setup).await
    }
    async fn result(&self, setup: &BotSetup) -> Result<SetupResult, BotError> {
        self.current_result(setup).await
    }
}
