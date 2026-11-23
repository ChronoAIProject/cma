use super::{
    nyxid::{complete, identifier, NyxidClient},
    EffectAdmission,
};
use layer2_core::bots::{Bot, BotError, BotKeyRef, BotKeyRole};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Keys {
    keys: Vec<Key>,
}
#[derive(Deserialize)]
pub(super) struct Key {
    pub id: String,
    pub name: String,
    pub is_active: bool,
    pub scopes: String,
    pub allowed_service_ids: Vec<String>,
    pub allowed_node_ids: Vec<String>,
    pub allow_all_services: bool,
    pub allow_all_nodes: bool,
    #[serde(default)]
    pub allow_auto_connected_services: bool,
    pub credential_source: Value,
    pub callback_url: Option<String>,
    pub expires_at: Option<String>,
    pub platform: Option<String>,
    pub purpose: String,
    pub scheduled_write_enabled: bool,
    #[serde(default)]
    pub bindings_count: u64,
}
pub(super) fn matches_expiry(observed: Option<&str>, requested: &str) -> bool {
    let Some(observed) = observed else {
        return false;
    };
    // Bot creation sends whole seconds. Refuse any fractional change before
    // BSON's millisecond parser can truncate it, but accept equivalent offsets.
    if observed.split_once('.').is_some_and(|(_, fraction)| {
        fraction
            .bytes()
            .take_while(u8::is_ascii_digit)
            .any(|digit| digit != b'0')
    }) {
        return false;
    }
    match (
        mongodb::bson::DateTime::parse_rfc3339_str(observed),
        mongodb::bson::DateTime::parse_rfc3339_str(requested),
    ) {
        (Ok(observed), Ok(requested)) => observed == requested,
        _ => false,
    }
}
pub(super) fn name(bot: &Bot, role: BotKeyRole) -> String {
    format!(
        "cma:bot:{}:{}:{}",
        bot.bot_id,
        match role {
            BotKeyRole::Agent => "agent",
            BotKeyRole::Route => "route",
        },
        bot.generation
    )
}
fn role(bot: &Bot, name: &str) -> Option<BotKeyRole> {
    let rest = name.strip_prefix(&format!("cma:bot:{}:", bot.bot_id))?;
    let (role, generation) = rest.split_once(':')?;
    let parsed: u64 = generation.parse().ok()?;
    if parsed == 0 || parsed > bot.generation || parsed.to_string() != generation {
        return None;
    }
    match role {
        "agent" => Some(BotKeyRole::Agent),
        "route" => Some(BotKeyRole::Route),
        _ => None,
    }
}
impl Key {
    fn require(&self, bot: &Bot) -> Result<BotKeyRole, BotError> {
        identifier(&self.id)?;
        let role = role(bot, &self.name).ok_or(BotError::Forbidden)?;
        if !crate::auth::keys::owner_matches(&self.credential_source, &bot.resource_owner())
            || self.scopes != "proxy read"
            || self.allow_all_services
            || self.allow_all_nodes
            || self.allow_auto_connected_services
            || self
                .allowed_node_ids
                .iter()
                .any(|id| identifier(id).is_err())
            || self
                .allowed_node_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.allowed_node_ids.len()
            || self.purpose != "general"
            || self.scheduled_write_enabled
            || self.bindings_count != 0
            || (role == BotKeyRole::Route
                && (!self.allowed_service_ids.is_empty()
                    || !self.allowed_node_ids.is_empty()
                    || self.platform.as_deref() != Some("generic")))
        {
            return Err(BotError::Forbidden);
        }
        Ok(role)
    }
}
impl NyxidClient {
    pub async fn owned_keys(&self, token: &str, bot: &Bot) -> Result<Vec<BotKeyRef>, BotError> {
        let path = match bot.resource_owner() {
            layer2_core::resource_access::ResourceOwner::Organization { subject } => {
                identifier(subject.as_str())?;
                format!("/api/v1/api-keys?org_id={subject}")
            }
            _ => "/api/v1/api-keys".into(),
        };
        let result: Keys = self.json(token, Method::GET, &path, None).await?;
        complete(
            result.keys.len() as u64,
            result.keys.iter().map(|k| k.id.as_str()),
        )?;
        let mut owned = Vec::new();
        for key in result
            .keys
            .into_iter()
            .filter(|k| k.is_active && role(bot, &k.name).is_some())
        {
            let role = key.require(bot)?;
            owned.push(BotKeyRef { id: key.id, role });
        }
        Ok(owned)
    }
    pub async fn owned_key(&self, token: &str, bot: &Bot, id: &str) -> Result<Key, BotError> {
        identifier(id)?;
        let key: Key = self
            .json(token, Method::GET, &format!("/api/v1/api-keys/{id}"), None)
            .await?;
        key.require(bot)?;
        if key.id != id {
            return Err(BotError::Forbidden);
        }
        Ok(key)
    }
    pub(super) async fn revoke_key(
        &self,
        token: &str,
        bot: &Bot,
        id: &str,
        admission: Option<&EffectAdmission>,
    ) -> Result<(), BotError> {
        // Listing omission is authoritative only after bounded complete enumeration.
        if !self
            .owned_keys(token, bot)
            .await?
            .iter()
            .any(|key| key.id == id)
        {
            return Ok(());
        }
        self.owned_key(token, bot, id).await?;
        if self
            .routes_for_bot(token, bot)
            .await?
            .iter()
            .any(|r| r.agent_api_key_id == id)
        {
            return Err(BotError::Forbidden);
        }
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        self.delete(token, &format!("/api/v1/api-keys/{id}")).await
    }
    pub(super) async fn callback(
        &self,
        token: &str,
        bot: &Bot,
        id: &str,
        url: &str,
        admission: Option<&EffectAdmission>,
    ) -> Result<(), BotError> {
        let key = self.owned_key(token, bot, id).await?;
        if key.require(bot)? != BotKeyRole::Route || !key.is_active {
            return Err(BotError::Forbidden);
        }
        if let Some(admission) = admission {
            admission.check(bot)?;
        }
        let updated: Key = self
            .json(
                token,
                Method::PUT,
                &format!("/api/v1/api-keys/{id}"),
                Some(json!({"callback_url":url})),
            )
            .await?;
        if updated.id != id
            || updated.callback_url.as_deref() != Some(url)
            || updated.require(bot)? != BotKeyRole::Route
        {
            return Err(BotError::Unavailable);
        }
        Ok(())
    }
}
