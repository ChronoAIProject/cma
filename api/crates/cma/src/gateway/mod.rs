//! Production Bot effects over the CMAEG and NyxID HTTP contracts.
mod client;
pub(crate) mod connections;
mod keys;
mod nyxid;
mod profile;
#[cfg(test)]
mod tests;
mod wire;
use crate::{
    auth::{keys::KeyIssuer, AuthState},
    config::GatewayConfig,
};
use layer2_core::{bots::*, sandbox::binding::ProvidedCredential, SubjectId};
use std::{sync::Arc, time::Duration};

mod effect_admission;
use effect_admission::EffectAdmission;

pub(crate) struct GatewayBots {
    auth: Arc<dyn OwnerTokens>,
    gateway: client::GatewayClient,
    nyxid: nyxid::NyxidClient,
    issuer: KeyIssuer,
    connections: connections::ChannelConnections,
    public: String,
}
#[async_trait::async_trait]
trait OwnerTokens: Send + Sync {
    async fn bot_owner_token(
        &self,
        owner: &SubjectId,
    ) -> Result<crate::auth::persistence::SecretValue, BotError>;
    async fn bot_authority_token(
        &self,
        bot: &Bot,
    ) -> Result<crate::auth::persistence::SecretValue, BotError> {
        self.bot_owner_token(bot.credential_subject()).await
    }
}
#[async_trait::async_trait]
impl OwnerTokens for AuthState {
    async fn bot_authority_token(
        &self,
        bot: &Bot,
    ) -> Result<crate::auth::persistence::SecretValue, BotError> {
        AuthState::bot_authority_token(self, bot).await
    }
    async fn bot_owner_token(
        &self,
        owner: &SubjectId,
    ) -> Result<crate::auth::persistence::SecretValue, BotError> {
        AuthState::bot_owner_token(self, owner).await
    }
}
impl GatewayBots {
    pub fn new(
        config: &GatewayConfig,
        auth: Arc<AuthState>,
        nyxid_url: &str,
        deployment: &str,
    ) -> Result<Arc<Self>, String> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(3))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .user_agent("cma-bots/1")
            .build()
            .map_err(|_| "cannot configure bot HTTP client")?;
        Ok(Arc::new(Self {
            auth,
            gateway: client::GatewayClient::new(
                http.clone(),
                config,
                nyxid_url.trim_end_matches('/').into(),
            )?,
            nyxid: nyxid::NyxidClient {
                http: http.clone(),
                base: nyxid_url.trim_end_matches('/').into(),
            },
            connections: connections::ChannelConnections::new(
                http.clone(),
                &config.base_url,
                &config.public_url,
            )?
            .with_public_api(&config.public_api_url)?
            .with_public_api_aliases(&config.public_api_aliases)?,
            issuer: KeyIssuer::new(http, nyxid_url, deployment),
            public: config.public_api_url.trim_end_matches('/').into(),
        }))
    }
}
async fn bounded<T>(
    future: impl std::future::Future<Output = Result<T, BotError>>,
) -> Result<T, BotError> {
    tokio::time::timeout(Duration::from_secs(60), future)
        .await
        .map_err(|_| BotError::Unavailable)?
}
/// Admission uses the deadline recorded before this call, never a fresh lease.
/// A timeout cancels local polling only; the durable reservation remains held.
async fn bounded_effect<T>(
    bot: &Bot,
    future: impl std::future::Future<Output = Result<T, BotError>>,
) -> Result<T, BotError> {
    let work = bot.work.as_ref().ok_or(BotError::Conflict)?;
    if work.effect_protocol == 0 {
        return bounded(future).await;
    }
    let effect = work.effect.as_ref().ok_or(BotError::Conflict)?;
    let seconds = layer2_core::seconds_from_rfc3339(&effect.deadline).ok_or(BotError::Conflict)?;
    let deadline = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(seconds);
    let remaining = deadline
        .duration_since(std::time::SystemTime::now())
        .map_err(|_| BotError::Conflict)?;
    if remaining.is_zero() {
        return Err(BotError::Conflict);
    }
    tokio::time::timeout(remaining.min(Duration::from_secs(60)), future)
        .await
        .map_err(|_| BotError::Unavailable)?
}
#[async_trait::async_trait]
impl BotExternal for GatewayBots {
    async fn available(&self, owner: &SubjectId) -> Result<Vec<ExternalBot>, BotError> {
        bounded(async {
            let token = self.auth.bot_owner_token(owner).await?;
            self.connections
                .list(
                    token.expose(),
                    &layer2_core::resource_access::ResourceOwner::Person {
                        subject: owner.clone(),
                    },
                )
                .await
                .map(|items| {
                    items
                        .into_iter()
                        .map(|item| ExternalBot {
                            id: item.connection_ref,
                            name: item.label.unwrap_or_else(|| "Channel".into()),
                            platform: String::new(),
                        })
                        .collect()
                })
        })
        .await
    }
    async fn verify(&self, bot: &Bot) -> Result<(), BotError> {
        bounded(async {
            let token = self.auth.bot_authority_token(bot).await?;
            profile::snapshot(bot)?;
            if let Some(reference) = &bot.request.connection_ref {
                let connection = self
                    .connections
                    .get(token.expose(), &bot.resource_owner(), reference)
                    .await?;
                return if connection.status == "ready" {
                    Ok(())
                } else {
                    Err(BotError::Conflict)
                };
            }
            self.nyxid.bot(token.expose(), bot).await?;
            if self
                .nyxid
                .routes_for_bot(token.expose(), bot)
                .await?
                .iter()
                .any(|r| {
                    r.channel_bot_id.as_deref() == Some(&bot.request.external_bot_id) && r.is_active
                })
            {
                return Err(BotError::Invalid(
                    "the selected external bot already has an active route".into(),
                ));
            }
            Ok(())
        })
        .await
    }
    async fn mint(&self, bot: &Bot, role: BotKeyRole) -> Result<BotKey, BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            let services = if role == BotKeyRole::Agent {
                profile::services(bot)?
            } else {
                vec![]
            };
            let pending = format!("{}/v1/callbacks/pending", self.public);
            if let Some(admission) = admission.as_ref() {
                admission.check(bot)?;
            }
            let key = self
                .issuer
                .mint_bot_admitted(
                    &keys::name(bot, role),
                    token.expose(),
                    &services,
                    (role == BotKeyRole::Route).then_some(pending.as_str()),
                    &bot.resource_owner(),
                    || {
                        admission
                            .as_ref()
                            .is_none_or(|admission| admission.check(bot).is_ok())
                    },
                )
                .await
                .map_err(|_| BotError::Unavailable)?;
            let observed = self.nyxid.owned_key(token.expose(), bot, &key.id).await?;
            let expires_at = layer2_core::rfc3339_at(key.expires_at);
            if !keys::matches_expiry(observed.expires_at.as_deref(), &expires_at)
                || !key.matches_grant(&observed.allowed_service_ids, &observed.allowed_node_ids)
            {
                return Err(BotError::Unavailable);
            }
            KeyIssuer::provided(key.value.expose()).map_err(|_| BotError::Unavailable)?;
            Ok(BotKey {
                id: key.id,
                value: ProvidedCredential::new(key.value.expose().to_owned())
                    .map_err(|_| BotError::Unavailable)?,
                expires_at,
            })
        })
        .await
    }
    async fn keys(&self, bot: &Bot) -> Result<Vec<BotKeyRef>, BotError> {
        bounded(async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.nyxid.owned_keys(token.expose(), bot).await
        })
        .await
    }
    async fn revoke(&self, bot: &Bot, id: &str) -> Result<(), BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.nyxid
                .revoke_key(token.expose(), bot, id, admission.as_ref())
                .await
        })
        .await
    }
    async fn create_channel(
        &self,
        bot: &Bot,
        secrets: &BotSecrets,
    ) -> Result<BotChannelObservation, BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            if bot.request.connection_ref.is_some() {
                return self
                    .gateway
                    .create_with_admission(token.expose(), bot, secrets, None, admission.as_ref())
                    .await;
            }
            let external = self.nyxid.bot(token.expose(), bot).await?;
            let username = (!external.platform_bot_username.is_empty())
                .then_some(external.platform_bot_username);
            self.gateway
                .create_with_admission(token.expose(), bot, secrets, username, admission.as_ref())
                .await
        })
        .await
    }
    async fn channel(&self, bot: &Bot) -> Result<Option<BotChannelObservation>, BotError> {
        bounded(async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.gateway.channel(token.expose(), bot).await
        })
        .await
    }
    async fn configure_channel(
        &self,
        bot: &Bot,
        current: &BotChannelObservation,
        keys: &BotCredentialSet,
        secrets: Option<&BotSecrets>,
        enabled: bool,
    ) -> Result<BotChannelObservation, BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.gateway
                .configure_with_admission(
                    token.expose(),
                    bot,
                    current,
                    keys,
                    secrets,
                    enabled,
                    admission.as_ref(),
                )
                .await
        })
        .await
    }
    async fn delete_channel(
        &self,
        bot: &Bot,
        current: &BotChannelObservation,
    ) -> Result<(), BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.gateway
                .delete_with_bot_admitted(token.expose(), Some(bot), current, admission.as_ref())
                .await
        })
        .await
    }
    async fn set_callback(&self, bot: &Bot, key_id: &str, url: &str) -> Result<(), BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            if bot.resources.channel.as_ref().is_none_or(|c| {
                c.callback_url != url || !self.gateway.valid_callback(&c.channel_id, url)
            }) {
                return Err(BotError::Forbidden);
            }
            self.nyxid
                .callback(token.expose(), bot, key_id, url, admission.as_ref())
                .await
        })
        .await
    }
    async fn create_route(&self, bot: &Bot, key_id: &str) -> Result<String, BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.nyxid.owned_key(token.expose(), bot, key_id).await?;
            self.nyxid
                .create_route(token.expose(), bot, key_id, admission.as_ref())
                .await
        })
        .await
    }
    async fn set_route_key(&self, bot: &Bot, route: &str, key: &str) -> Result<(), BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            let owned: Vec<_> = self
                .nyxid
                .owned_keys(token.expose(), bot)
                .await?
                .into_iter()
                .filter(|k| k.role == BotKeyRole::Route)
                .map(|k| k.id)
                .collect();
            if !owned.iter().any(|id| id == key) {
                return Err(BotError::Forbidden);
            }
            self.nyxid
                .set_route_key(token.expose(), bot, route, key, &owned, admission.as_ref())
                .await
        })
        .await
    }
    async fn active_route_key(&self, bot: &Bot) -> Result<Option<String>, BotError> {
        bounded(async {
            let token = self.auth.bot_authority_token(bot).await?;
            let owned: Vec<_> = self
                .nyxid
                .owned_keys(token.expose(), bot)
                .await?
                .into_iter()
                .filter(|key| key.role == BotKeyRole::Route)
                .map(|key| key.id)
                .collect();
            self.nyxid
                .active_route_key(token.expose(), bot, &owned)
                .await
        })
        .await
    }
    async fn delete_routes(&self, bot: &Bot, key_ids: &[String]) -> Result<(), BotError> {
        let admission = EffectAdmission::from_bot(bot)?;
        bounded_effect(bot, async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.nyxid
                .delete_routes(token.expose(), bot, key_ids, admission.as_ref())
                .await
        })
        .await
    }
    async fn test(&self, bot: &Bot) -> Result<BotTestResult, BotError> {
        bounded(async {
            let token = self.auth.bot_authority_token(bot).await?;
            self.gateway.test(token.expose(), bot).await
        })
        .await
    }
}
