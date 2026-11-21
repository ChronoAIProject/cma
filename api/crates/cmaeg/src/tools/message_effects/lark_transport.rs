//! Existing public NyxID bot-service proxy; no relay mutation route is invented.
use super::lark_ledger::Reaction;
use crate::{
    delivery::intents::DeliveryOutcome,
    media::{
        body::{read_capped, Overflow},
        source::lark,
    },
};
use cmaeg_core::{
    channel::{lark_resources::safe_id, Secret},
    seal::{lark_effect::LarkEffectAuthority, MessageRefPayload},
};
use reqwest::{Client, Method, Url};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::{
    sync::Semaphore,
    time::{timeout_at, Instant},
};

pub struct LarkEffectsClient {
    http: Client,
    base: Url,
    pub(super) capacity: Semaphore,
}
impl LarkEffectsClient {
    pub fn new(base: &str) -> Result<Self, crate::ConfigError> {
        let origin = crate::config::validate_origin(crate::CMAEG_NYXID_BASE_URL, base)?;
        Ok(Self {
            http: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()
                .map_err(|_| crate::ConfigError::InvalidManagement {
                    detail: "Lark effects client".into(),
                })?,
            base: Url::parse(&origin).map_err(|_| crate::ConfigError::InvalidManagement {
                detail: "Lark effects origin".into(),
            })?,
            capacity: Semaphore::new(32),
        })
    }
    pub(super) async fn authorize(
        &self,
        key: &Secret,
        message: &MessageRefPayload,
        deadline: Instant,
    ) -> Result<(), DeliveryOutcome> {
        let a = message.lark_effect.as_ref().expect("checked authority");
        lark::authorize_connection(
            &self.http,
            &self.base,
            a.platform,
            &a.binding.connection,
            &a.bot_id,
            key,
            deadline,
        )
        .await
        .map_err(|_| rejected("lark_connection_unavailable"))?;
        let data = self
            .request(
                key,
                a,
                Method::GET,
                &message.platform_message_id,
                &[],
                None,
                deadline,
            )
            .await
            .map_err(|_| rejected("lark_message_unverified"))?;
        let rows = data
            .pointer("/data/items")
            .and_then(Value::as_array)
            .filter(|rows| rows.len() == 1)
            .ok_or_else(|| rejected("lark_message_unverified"))?;
        let row = &rows[0];
        let same_sender = row.pointer("/sender/sender_type").and_then(Value::as_str) == Some("app")
            && ((row.pointer("/sender/id_type").and_then(Value::as_str) == Some("app_id")
                && row.pointer("/sender/id").and_then(Value::as_str)
                    == Some(&a.binding.connection.app_id))
                || row.pointer("/sender/open_bot_id").and_then(Value::as_str) == Some(&a.bot_id));
        if row.get("message_id").and_then(Value::as_str) != Some(&message.platform_message_id)
            || row.get("chat_id").and_then(Value::as_str) != Some(&a.chat_id)
            || row
                .get("thread_id")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                != a.thread_id.as_deref()
            || !same_sender
            || row.get("deleted") == Some(&Value::Bool(true))
        {
            return Err(rejected("lark_message_unverified"));
        }
        Ok(())
    }
    pub(super) async fn delete(
        &self,
        key: &Secret,
        message: &MessageRefPayload,
        deadline: Instant,
    ) -> Result<(), DeliveryOutcome> {
        self.request(
            key,
            message.lark_effect.as_ref().expect("checked authority"),
            Method::DELETE,
            &message.platform_message_id,
            &[],
            None,
            deadline,
        )
        .await?;
        Ok(())
    }
    pub(super) async fn reaction(
        &self,
        key: &Secret,
        message: &MessageRefPayload,
        previous: Option<&Reaction>,
        desired: Option<&str>,
        deadline: Instant,
    ) -> Result<Option<Reaction>, DeliveryOutcome> {
        let a = message.lark_effect.as_ref().expect("checked authority");
        if let Some(previous) = previous {
            let result = self
                .request(
                    key,
                    a,
                    Method::DELETE,
                    &message.platform_message_id,
                    &["reactions", &previous.id],
                    None,
                    deadline,
                )
                .await?;
            if reaction_receipt(&result, a, &previous.emoji)
                .is_none_or(|reaction| reaction.id != previous.id)
            {
                return Err(unknown());
            }
        }
        let Some(emoji) = desired else {
            return Ok(None);
        };
        let result = self
            .request(
                key,
                a,
                Method::POST,
                &message.platform_message_id,
                &["reactions"],
                Some(json!({"reaction_type":{"emoji_type":emoji}})),
                deadline,
            )
            .await
            .map_err(|error| if previous.is_some() { unknown() } else { error })?;
        reaction_receipt(&result, a, emoji)
            .map(Some)
            .ok_or_else(unknown)
    }
    #[allow(clippy::too_many_arguments)]
    async fn request(
        &self,
        key: &Secret,
        authority: &LarkEffectAuthority,
        method: Method,
        message: &str,
        suffix: &[&str],
        body: Option<Value>,
        deadline: Instant,
    ) -> Result<Value, DeliveryOutcome> {
        let mut path = vec!["open-apis", "im", "v1", "messages", message];
        path.extend_from_slice(suffix);
        let url = lark::proxy_url(&self.base, &authority.binding.connection, &path, None)
            .map_err(|_| rejected("lark_request_invalid"))?;
        let mut request = self.http.request(method, url).bearer_auth(key.as_str());
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = timeout_at(deadline, request.send())
            .await
            .map_err(|_| unknown())?
            .map_err(|_| unknown())?;
        if !response.status().is_success() {
            return Err(unknown());
        }
        let body = read_capped(response, 128 * 1024, Overflow::Reject, deadline)
            .await
            .map_err(|_| unknown())?;
        let value: Value = serde_json::from_slice(&body.bytes).map_err(|_| unknown())?;
        match value.get("code").and_then(Value::as_i64) {
            Some(0) => Ok(value),
            Some(_) => Err(rejected("lark_request_rejected")),
            None => Err(unknown()),
        }
    }
}
fn reaction_receipt(
    value: &Value,
    authority: &LarkEffectAuthority,
    emoji: &str,
) -> Option<Reaction> {
    let data = value.get("data")?;
    let id = data.get("reaction_id")?.as_str()?.to_owned();
    let operator = data.pointer("/operator/operator_id")?.as_str()?;
    (safe_id(&id, 512)
        && data.pointer("/operator/operator_type")?.as_str()? == "app"
        && (operator == authority.binding.connection.app_id || operator == authority.bot_id)
        && data.pointer("/reaction_type/emoji_type")?.as_str()? == emoji)
        .then(|| Reaction {
            id,
            emoji: emoji.into(),
        })
}
pub(super) fn unknown() -> DeliveryOutcome {
    DeliveryOutcome::Unknown {
        reason: "lark_effect_unconfirmed".into(),
    }
}
fn rejected(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: reason.into(),
    }
}
