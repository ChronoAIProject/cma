use super::{profile, wire::*, EffectAdmission};
use crate::{auth::provider_http::read_json, config::GatewayConfig};
use layer2_core::bots::*;
use reqwest::{Client, Method, Response, Url};
use serde::Serialize;
use serde_json::json;

pub(super) struct GatewayClient {
    http: Client,
    base: Url,
    public: Url,
    public_api: Url,
    public_api_aliases: Vec<Url>,
    issuer: String,
}
impl GatewayClient {
    pub fn new(http: Client, config: &GatewayConfig, issuer: String) -> Result<Self, String> {
        Ok(Self {
            http,
            base: Url::parse(&config.base_url).map_err(|_| "invalid gateway origin")?,
            public: Url::parse(&config.public_url).map_err(|_| "invalid gateway public origin")?,
            public_api: Url::parse(&config.public_api_url)
                .map_err(|_| "invalid gateway public API base")?,
            public_api_aliases: config
                .public_api_aliases
                .iter()
                .map(|base| {
                    Url::parse(base).map_err(|_| "invalid gateway public API alias".to_owned())
                })
                .collect::<Result<_, _>>()?,
            issuer,
        })
    }
    pub(super) fn valid_callback(&self, channel: &str, value: &str) -> bool {
        [&self.public_api, &self.public]
            .into_iter()
            .chain(self.public_api_aliases.iter())
            .any(|base| {
                endpoint(base, "callbacks", Some(channel), None)
                    .is_ok_and(|url| value == url.as_str())
            })
    }

    fn url(&self, collection: &str, id: Option<&str>) -> Result<Url, BotError> {
        endpoint(&self.base, collection, id, None)
    }
    async fn request<T: Serialize + ?Sized>(
        &self,
        token: &str,
        bot: &Bot,
        method: Method,
        url: Url,
        body: Option<&T>,
    ) -> Result<Response, BotError> {
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let layer2_core::resource_access::ResourceOwner::Organization { subject } =
            bot.resource_owner()
        {
            request = request.header("X-CMAEG-Owner", subject.as_str());
        }
        let request = if let Some(body) = body {
            request.json(body)
        } else {
            request
        };
        let response = request.send().await.map_err(|_| BotError::Unavailable)?;
        if response
            .headers()
            .get("cmaeg-protocol-version")
            .and_then(|v| v.to_str().ok())
            != Some("1")
        {
            return Err(BotError::Unavailable);
        }
        Ok(response)
    }
    fn policy(
        &self,
        bot: &Bot,
        key: &str,
        routes: Vec<String>,
        username: Option<String>,
        enabled: bool,
    ) -> Result<Policy, BotError> {
        Ok(Policy {
            schema_version: 1,
            name: bot.request.name.clone(),
            profile: profile::snapshot(bot)?,
            provider: ProviderRequest {
                slug: bot.provider.clone(),
            },
            connection_ref: bot.request.connection_ref.clone(),
            connection_admission: bot
                .request
                .connection_ref
                .as_ref()
                .map(|_| admission(bot, enabled)),
            sources: if bot.request.connection_ref.is_some() {
                vec![]
            } else {
                vec![SourceRequest {
                    kind: "nyxid_relay",
                    issuer: self.issuer.clone(),
                    key_id: key.into(),
                    route_ids: routes,
                    platform: "telegram",
                    bot_username: username,
                    admission: if enabled {
                        bot.request.admission.clone()
                    } else {
                        BotAdmission::Scoped {
                            senders: IdentityAdmission::Deny,
                            chats: IdentityAdmission::Deny,
                            groups: GroupAdmission::Deny,
                        }
                    },
                }]
            },
            partition: json!({"version":1,"key":"conversation_and_sender"}),
            reply: reply(&bot.request.reply),
        })
    }
    #[cfg(test)]
    pub async fn create(
        &self,
        token: &str,
        bot: &Bot,
        secrets: &BotSecrets,
        username: Option<String>,
    ) -> Result<BotChannelObservation, BotError> {
        self.create_with_admission(token, bot, secrets, username, None)
            .await
    }
    pub(super) async fn create_with_admission(
        &self,
        token: &str,
        bot: &Bot,
        secrets: &BotSecrets,
        username: Option<String>,
        admission: Option<&EffectAdmission>,
    ) -> Result<BotChannelObservation, BotError> {
        let key = route_key(bot, &bot.resources.credentials)?;
        let body = Create {
            policy: self.policy(bot, key, vec![], username, true)?,
            record_id: &bot.record_id,
            credentials: credentials(secrets),
        };
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        let response = self
            .request(
                token,
                bot,
                Method::POST,
                self.url("channels", None)?,
                Some(&body),
            )
            .await?;
        if response.status() != 201 {
            return Err(BotError::Unavailable);
        }
        let channel: Channel = read_json(response)
            .await
            .map_err(|_| BotError::Unavailable)?;
        if channel
            .definition
            .creation
            .as_ref()
            .is_none_or(|c| !c.ready)
            || !channel.provider.refused.is_empty()
        {
            return Err(BotError::Invalid(
                "gateway provider refused the requested profile".into(),
            ));
        }
        self.observation(bot, channel)
    }
    async fn read(&self, token: &str, bot: &Bot) -> Result<Option<Channel>, BotError> {
        let response = self
            .request::<()>(
                token,
                bot,
                Method::GET,
                self.url("channel-records", Some(&bot.record_id))?,
                None,
            )
            .await?;
        if response.status() == 404 {
            return Ok(None);
        }
        read_json(response)
            .await
            .map(Some)
            .map_err(|_| BotError::Unavailable)
    }
    pub async fn channel(
        &self,
        token: &str,
        bot: &Bot,
    ) -> Result<Option<BotChannelObservation>, BotError> {
        self.read(token, bot)
            .await?
            .map(|c| self.observation(bot, c))
            .transpose()
    }
    #[cfg(test)]
    pub async fn configure(
        &self,
        token: &str,
        bot: &Bot,
        current: &BotChannelObservation,
        keys: &BotCredentialSet,
        secrets: Option<&BotSecrets>,
        enabled: bool,
    ) -> Result<BotChannelObservation, BotError> {
        self.configure_with_admission(token, bot, current, keys, secrets, enabled, None)
            .await
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn configure_with_admission(
        &self,
        token: &str,
        bot: &Bot,
        current: &BotChannelObservation,
        keys: &BotCredentialSet,
        secrets: Option<&BotSecrets>,
        enabled: bool,
        admission: Option<&EffectAdmission>,
    ) -> Result<BotChannelObservation, BotError> {
        let channel = self.read(token, bot).await?.ok_or(BotError::Unavailable)?;
        let username = channel
            .definition
            .sources
            .first()
            .and_then(|s| s.bot_username.clone());
        let observed = self.observation(bot, channel)?;
        if observed.channel != current.channel {
            return Err(BotError::Unavailable);
        }
        let key = route_key(bot, keys)?;
        let body = Update {
            policy: self.policy(
                bot,
                key,
                bot.resources.route_id.iter().cloned().collect(),
                username,
                enabled,
            )?,
            expected_version: current.channel.version,
            credentials: secrets.map(credentials),
        };
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        let response = self
            .request(
                token,
                bot,
                Method::PUT,
                self.url("channels", Some(&current.channel.channel_id))?,
                Some(&body),
            )
            .await?;
        let channel: Channel = read_json(response)
            .await
            .map_err(|_| BotError::Unavailable)?;
        if !channel.provider.refused.is_empty() {
            return Err(BotError::Invalid(
                "gateway provider refused the requested profile".into(),
            ));
        }
        let updated = self.observation(bot, channel)?;
        if !updated.uses(keys)
            || updated.channel.version <= current.channel.version
            || updated.route_ids != bot.resources.route_id.iter().cloned().collect::<Vec<_>>()
        {
            return Err(BotError::Unavailable);
        }
        Ok(updated)
    }
    #[allow(dead_code)] // Retained for Bot cleanup retries after a failed setup.
    pub async fn delete(
        &self,
        token: &str,
        current: &BotChannelObservation,
    ) -> Result<(), BotError> {
        self.delete_with_bot(token, None, current).await
    }
    pub async fn delete_with_bot(
        &self,
        token: &str,
        bot: Option<&Bot>,
        current: &BotChannelObservation,
    ) -> Result<(), BotError> {
        self.delete_with_bot_admitted(token, bot, current, None)
            .await
    }
    pub(super) async fn delete_with_bot_admitted(
        &self,
        token: &str,
        bot: Option<&Bot>,
        current: &BotChannelObservation,
        admission: Option<&EffectAdmission>,
    ) -> Result<(), BotError> {
        let mut url = self.url("channels", Some(&current.channel.channel_id))?;
        url.query_pairs_mut()
            .append_pair("expected_version", &current.channel.version.to_string());
        if let (Some(bot), Some(admission)) = (bot, admission) {
            admission.check(bot)?;
        }
        let response = if let Some(bot) = bot {
            self.request::<()>(token, bot, Method::DELETE, url, None)
                .await?
        } else {
            self.http
                .delete(url)
                .bearer_auth(token)
                .send()
                .await
                .map_err(|_| BotError::Unavailable)?
        };
        if response.status() == 204 || response.status() == 404 {
            Ok(())
        } else {
            Err(BotError::Unavailable)
        }
    }
    pub async fn test(&self, token: &str, bot: &Bot) -> Result<BotTestResult, BotError> {
        let channel = bot.resources.channel.as_ref().ok_or(BotError::Conflict)?;
        let url = endpoint(
            &self.base,
            "channels",
            Some(&channel.channel_id),
            Some("test"),
        )?;
        let response = self
            .request::<()>(token, bot, Method::POST, url, None)
            .await?;
        let result: Test = read_json(response)
            .await
            .map_err(|_| BotError::Unavailable)?;
        if !level(&result.level) {
            return Err(BotError::Unavailable);
        }
        Ok(BotTestResult {
            accepted_after_ms: result.accepted_after_ms,
            first_text: result.first_text,
            level: result.level,
            tested_at: layer2_core::now_rfc3339(),
        })
    }
    fn observation(&self, bot: &Bot, channel: Channel) -> Result<BotChannelObservation, BotError> {
        let def = channel.definition;
        if channel.record_id != bot.record_id
            || def.record_id != bot.record_id
            || def.owner.subject != bot.owner.as_str()
            || def.owner.kind
                != match bot.resource_owner() {
                    layer2_core::resource_access::ResourceOwner::Person { .. } => "human",
                    layer2_core::resource_access::ResourceOwner::Organization { .. } => {
                        "organization"
                    }
                }
            || channel.provider.slug != bot.provider
            || !level(&channel.provider.level)
            || channel.version == 0
            || channel.channel_id.is_empty()
            || channel.channel_id.len() > 512
            || (bot.request.connection_ref.is_none() && def.sources.len() != 1)
            || def.connection_ref != bot.request.connection_ref
            || def.profile != profile::snapshot(bot)?
        {
            return Err(BotError::Unavailable);
        }
        let (route_key_id, route_ids) = if bot.request.connection_ref.is_some() {
            (String::new(), vec![])
        } else {
            let source = def
                .sources
                .into_iter()
                .next()
                .ok_or(BotError::Unavailable)?;
            if source.kind != "nyxid_relay"
                || source.issuer != self.issuer
                || !matches!(source.platform.as_str(), "telegram" | "telegram-new")
            {
                return Err(BotError::Unavailable);
            }
            (source.key_id, source.route_ids)
        };
        let allowed = [&self.public_api, &self.public]
            .into_iter()
            .chain(self.public_api_aliases.iter())
            .any(|base| {
                endpoint(base, "callbacks", Some(&channel.channel_id), None)
                    .is_ok_and(|url| channel.endpoints.nyxid_callback_url == url.as_str())
                    && endpoint(base, "channels", Some(&channel.channel_id), Some("events"))
                        .is_ok_and(|url| channel.endpoints.custom_event_url == url.as_str())
            });
        if !allowed {
            return Err(BotError::Unavailable);
        }
        Ok(BotChannelObservation {
            channel: BotChannel {
                channel_id: channel.channel_id,
                record_id: channel.record_id,
                version: channel.version,
                binding_id: channel.provider.binding_id,
                level: channel.provider.level,
                callback_url: channel.endpoints.nyxid_callback_url,
                event_url: channel.endpoints.custom_event_url,
            },
            agent_key_hash: def
                .secrets
                .agent_key_hash
                .strip_prefix("sha256:")
                .ok_or(BotError::Unavailable)?
                .to_owned(),
            route_key_id,
            route_ids,
        })
    }
}
fn credentials(secrets: &BotSecrets) -> Credentials<'_> {
    Credentials {
        agent_key: secrets.agent.expose(),
        channel_key: secrets.route.as_ref().map(|value| value.expose()),
    }
}
fn route_key<'a>(bot: &Bot, keys: &'a BotCredentialSet) -> Result<&'a str, BotError> {
    if bot.request.connection_ref.is_some() {
        if keys.route_key_id.is_some() || keys.route_key_hash.is_some() {
            return Err(BotError::Conflict);
        }
        Ok("")
    } else {
        keys.route_key_id.as_deref().ok_or(BotError::Unavailable)
    }
}
fn level(value: &str) -> bool {
    matches!(value, "l1" | "l2" | "l3" | "l4" | "l5")
}
fn endpoint(
    base: &Url,
    collection: &str,
    id: Option<&str>,
    suffix: Option<&str>,
) -> Result<Url, BotError> {
    let mut url = base.clone();
    {
        let mut path = url.path_segments_mut().map_err(|_| BotError::Unavailable)?;
        path.pop_if_empty().push("v1").push(collection);
        if let Some(id) = id {
            if id == "." || id == ".." {
                return Err(BotError::Unavailable);
            }
            path.push(id);
        }
        if let Some(suffix) = suffix {
            path.push(suffix);
        }
    }
    Ok(url)
}

fn admission(bot: &Bot, enabled: bool) -> BotAdmission {
    if enabled {
        bot.request.admission.clone()
    } else {
        BotAdmission::Scoped {
            senders: IdentityAdmission::Deny,
            chats: IdentityAdmission::Deny,
            groups: GroupAdmission::Deny,
        }
    }
}
