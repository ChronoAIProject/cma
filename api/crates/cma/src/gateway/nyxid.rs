//! Owner-only channel metadata and bounded route mutations.
use super::EffectAdmission;
use crate::auth::{keys::valid_provider_key_id, provider_http::read_json};
use layer2_core::bots::{Bot, BotError};
use reqwest::{Client, Method};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

pub(super) struct NyxidClient {
    pub http: Client,
    pub base: String,
}
#[derive(Deserialize)]
pub(super) struct ChannelBot {
    pub id: String,
    pub user_id: String,
    pub platform: String,
    pub platform_bot_username: String,
    pub is_active: bool,
}
#[derive(Deserialize)]
pub(super) struct Routes {
    pub conversations: Vec<Route>,
    pub total: u64,
}
#[derive(Deserialize)]
pub(super) struct Route {
    pub id: String,
    pub channel_bot_id: Option<String>,
    pub platform: String,
    pub agent_api_key_id: String,
    pub platform_conversation_id: String,
    pub default_agent: bool,
    pub is_active: bool,
}
impl NyxidClient {
    pub async fn json<T: DeserializeOwned>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, BotError> {
        let request = self
            .http
            .request(method, format!("{}{}", self.base, path))
            .bearer_auth(token);
        let request = if let Some(body) = body {
            request.json(&body)
        } else {
            request
        };
        read_json(request.send().await.map_err(|_| BotError::Unavailable)?)
            .await
            .map_err(|_| BotError::Unavailable)
    }
    pub async fn bot(&self, token: &str, bot: &Bot) -> Result<ChannelBot, BotError> {
        identifier(&bot.request.external_bot_id)?;
        let result: ChannelBot = self
            .json(
                token,
                Method::GET,
                &format!("/api/v1/channel-bots/{}", bot.request.external_bot_id),
                None,
            )
            .await?;
        if result.id != bot.request.external_bot_id
            || result.user_id != bot.owner.as_str()
            || !result.is_active
            || !matches!(result.platform.as_str(), "telegram" | "telegram-new")
        {
            return Err(BotError::Forbidden);
        }
        Ok(result)
    }
    #[allow(dead_code)] // Used by the route cleanup/reconciliation worker.
    pub async fn routes(&self, token: &str) -> Result<Vec<Route>, BotError> {
        self.routes_at(token, "/api/v1/channel-conversations").await
    }
    pub async fn routes_for_bot(&self, token: &str, bot: &Bot) -> Result<Vec<Route>, BotError> {
        let path = match bot.resource_owner() {
            layer2_core::resource_access::ResourceOwner::Organization { subject } => {
                identifier(subject.as_str())?;
                format!("/api/v1/channel-conversations?org_id={subject}")
            }
            _ => "/api/v1/channel-conversations".into(),
        };
        self.routes_at(token, &path).await
    }
    async fn routes_at(&self, token: &str, path: &str) -> Result<Vec<Route>, BotError> {
        let result: Routes = self.json(token, Method::GET, path, None).await?;
        complete(
            result.total,
            result.conversations.iter().map(|r| r.id.as_str()),
        )?;
        Ok(result.conversations)
    }
    pub(super) async fn create_route(
        &self,
        token: &str,
        bot: &Bot,
        key: &str,
        admission: Option<&EffectAdmission>,
    ) -> Result<String, BotError> {
        // No route owned by another integration may be silently overridden.
        if self.routes_for_bot(token, bot).await?.iter().any(|r| {
            r.channel_bot_id.as_deref() == Some(&bot.request.external_bot_id) && r.is_active
        }) {
            return Err(BotError::Invalid(
                "the selected external bot already has an active route".into(),
            ));
        }
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        let mut body = json!({
            "channel_bot_id":bot.request.external_bot_id,"agent_api_key_id":key,"default_agent":true,"platform_conversation_id":"*"
        });
        if let layer2_core::resource_access::ResourceOwner::Organization { subject } =
            bot.resource_owner()
        {
            body["target_org_id"] = subject.as_str().into();
        }
        let route: Route = self
            .json(
                token,
                Method::POST,
                "/api/v1/channel-conversations",
                Some(body),
            )
            .await?;
        route.require(bot, &[key.to_owned()])?;
        Ok(route.id)
    }
    pub(super) async fn set_route_key(
        &self,
        token: &str,
        bot: &Bot,
        route: &str,
        key: &str,
        owned: &[String],
        admission: Option<&EffectAdmission>,
    ) -> Result<(), BotError> {
        identifier(route)?;
        let routes = self.routes_for_bot(token, bot).await?;
        let current = routes
            .iter()
            .find(|r| r.id == route)
            .ok_or(BotError::Unavailable)?;
        current.require(bot, owned)?;
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        let updated: Route = self
            .json(
                token,
                Method::PUT,
                &format!("/api/v1/channel-conversations/{route}"),
                Some(json!({"agent_api_key_id":key})),
            )
            .await?;
        updated.require(bot, &[key.to_owned()])
    }
    pub async fn active_route_key(
        &self,
        token: &str,
        bot: &Bot,
        owned: &[String],
    ) -> Result<Option<String>, BotError> {
        let expected = bot
            .resources
            .route_id
            .as_deref()
            .ok_or(BotError::Unavailable)?;
        identifier(expected)?;
        let routes = self.routes_for_bot(token, bot).await?;
        let mut retained = None;
        for route in routes {
            if route.id == expected {
                route.require(bot, owned)?;
                if !route.is_active {
                    return Err(BotError::Unavailable);
                }
                retained = Some(route.agent_api_key_id);
            } else if owned.contains(&route.agent_api_key_id)
                || route.channel_bot_id.as_deref() == Some(&bot.request.external_bot_id)
            {
                return Err(BotError::Forbidden);
            }
        }
        Ok(retained)
    }
    pub(super) async fn delete_routes(
        &self,
        token: &str,
        bot: &Bot,
        keys: &[String],
        admission: Option<&EffectAdmission>,
    ) -> Result<(), BotError> {
        let routes = self.routes_for_bot(token, bot).await?;
        let selected: Vec<_> = routes
            .iter()
            .filter(|r| keys.contains(&r.agent_api_key_id))
            .collect();
        for route in &selected {
            route.require(bot, keys)?;
        }
        for route in selected {
            if let Some(admission) = admission {
                admission.check(bot)?;
            }
            self.delete(
                token,
                &format!("/api/v1/channel-conversations/{}", route.id),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn delete(&self, token: &str, path: &str) -> Result<(), BotError> {
        let response = self
            .http
            .delete(format!("{}{}", self.base, path))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| BotError::Unavailable)?;
        if response.status().is_success() || response.status() == 404 {
            Ok(())
        } else {
            Err(BotError::Unavailable)
        }
    }
}
impl Route {
    pub fn require(&self, bot: &Bot, keys: &[String]) -> Result<(), BotError> {
        identifier(&self.id)?;
        if self.channel_bot_id.as_deref() != Some(&bot.request.external_bot_id)
            || !matches!(self.platform.as_str(), "telegram" | "telegram-new")
            || !keys.contains(&self.agent_api_key_id)
            || self.platform_conversation_id != "*"
            || !self.default_agent
        {
            return Err(BotError::Invalid(
                "route ownership or routing conflicts with this bot".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn identifier(value: &str) -> Result<(), BotError> {
    if valid_provider_key_id(value) && value != "." && value != ".." {
        Ok(())
    } else {
        Err(BotError::Unavailable)
    }
}
pub(super) fn complete<'a>(total: u64, ids: impl Iterator<Item = &'a str>) -> Result<(), BotError> {
    let mut seen = std::collections::BTreeSet::new();
    for id in ids {
        identifier(id)?;
        if !seen.insert(id) {
            return Err(BotError::Unavailable);
        }
    }
    if total as usize != seen.len() || total > 1000 {
        Err(BotError::Unavailable)
    } else {
        Ok(())
    }
}
