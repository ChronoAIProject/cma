//! Normalize platform creation receipts without inferring application consent.
use super::{
    facts::{Observation, RelayFacts},
    payload::CallbackPayload,
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    cloudevent::CloudEvent,
};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Called only after the ordinary Telegram observer verifies envelope identity,
/// original message, human sender, system-sender and attachment exclusions.
pub fn observe(
    payload: &CallbackPayload,
    raw: &Value,
    message: &Value,
) -> Result<Observation, VerificationError> {
    let denied = || VerificationError("telegram_managed_creation_invalid");
    let unsupported = || Ok(Observation::Refused(Refusal::ContentUnsupported));
    if message["chat"]["type"] != "private"
        || payload.conversation.platform_id != payload.sender.platform_id
        || payload.content.kind != "unknown"
        || payload.content.text.is_some()
        || raw.as_object().is_none_or(|object| {
            object
                .keys()
                .any(|key| !matches!(key.as_str(), "update_id" | "message"))
        })
        || message.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "message_id" | "date" | "chat" | "from" | "managed_bot_created"
                )
            })
        })
    {
        return unsupported();
    }
    let creator_id = positive_id(&message["from"]["id"]).ok_or_else(denied)?;
    let update_id = raw["update_id"]
        .as_u64()
        .filter(|id| *id <= i64::MAX as u64)
        .ok_or_else(denied)?;
    let message_id = positive_id(&message["message_id"]).ok_or_else(denied)?;
    let date = message["date"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(denied)?;
    let time = OffsetDateTime::from_unix_timestamp(date)
        .map_err(|_| denied())?
        .format(&Rfc3339)
        .map_err(|_| denied())?;
    let bot = &message["managed_bot_created"]["bot"];
    let bot_id = positive_id(&bot["id"]).ok_or_else(denied)?;
    if bot["is_bot"] != true {
        return Err(denied());
    }
    let first_name = bot["first_name"]
        .as_str()
        .filter(|name| !name.is_empty() && name.len() <= 256 && !name.chars().any(char::is_control))
        .ok_or_else(denied)?;
    let mut safe_bot = json!({"id":bot_id.to_string(),"first_name":first_name});
    if let Some(value) = bot.get("username") {
        let username = value
            .as_str()
            .filter(|name| {
                (5..=32).contains(&name.len())
                    && name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
            .ok_or_else(denied)?;
        safe_bot["username"] = username.into();
    }
    let ce = CloudEvent {
        specversion: "1.0".into(),
        id: update_id.to_string(),
        source: format!("nyxid:route:{}", payload.conversation.id),
        r#type: "telegram.managed_bot_created".into(),
        time: Some(time),
        subject: Some(format!("telegram:bot:{bot_id}")),
        datacontenttype: Some("application/json".into()),
        dataschema: None,
        data: Some(json!({
            "update_id":update_id.to_string(),
            "message_id":message_id.to_string(),
            "creator_id":creator_id.to_string(),
            "bot":safe_bot,
        })),
        extensions: Default::default(),
    };
    Ok(Observation::Message(RelayFacts {
        callback: None,
        event: Some(Box::new(ce)),
        conversation: ConversationKind::Private,
        thread_id: None,
        mentioned: Some(false),
        replied_to_bot: Some(false),
        actor: ActorKind::Human,
    }))
}

fn positive_id(value: &Value) -> Option<i64> {
    value.as_i64().filter(|value| *value > 0)
}
