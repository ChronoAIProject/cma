//! Telegram facts taken from authenticated raw fields and correlated to the envelope.
use super::{payload::CallbackPayload, VerificationError};
use crate::ingress::admission::Refusal;
use cmaeg_core::{
    activity::ConversationKind,
    channel::{AdmissionPolicy, GroupAdmission},
};
use serde_json::Value;

#[derive(Clone)]
pub struct CallbackFacts {
    pub id: String,
    pub data: String,
    pub message_id: String,
}
pub struct TelegramFacts {
    pub callback: Option<CallbackFacts>,
    pub conversation: ConversationKind,
    pub thread_id: Option<String>,
    pub mentioned: Option<bool>,
    pub replied_to_bot: Option<bool>,
}
impl TelegramFacts {
    pub fn addressed(&self, policy: &AdmissionPolicy) -> Option<bool> {
        match policy {
            AdmissionPolicy::Scoped {
                groups: GroupAdmission::Mention,
                ..
            } => self.mentioned,
            AdmissionPolicy::Scoped {
                groups: GroupAdmission::ReplyToBot,
                ..
            } => self.replied_to_bot,
            _ => match (self.mentioned, self.replied_to_bot) {
                (Some(true), _) | (_, Some(true)) => Some(true),
                (Some(false), Some(false)) => Some(false),
                _ => None,
            },
        }
    }
}
pub enum Observation {
    Message(TelegramFacts),
    Refused(Refusal),
}
pub fn observe(
    payload: &CallbackPayload,
    bot_username: Option<&str>,
) -> Result<Observation, VerificationError> {
    let Some(raw) = payload
        .raw_platform_data
        .as_ref()
        .filter(|raw| raw.is_object())
    else {
        return Ok(Observation::Refused(Refusal::UnknownSender));
    };
    let events: Vec<_> = [
        "message",
        "edited_message",
        "channel_post",
        "edited_channel_post",
    ]
    .into_iter()
    .filter_map(|kind| raw.get(kind).map(|message| (kind, message)))
    .collect();
    if events.len() > 1 || (!events.is_empty() && raw.get("callback_query").is_some()) {
        return Err(VerificationError("telegram_event_ambiguous"));
    }
    if let Some(callback) = raw.get("callback_query") {
        return observe_callback(payload, callback);
    }
    let Some((kind, message)) = events.first() else {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    };
    let chat = message
        .get("chat")
        .ok_or(VerificationError("telegram_chat_missing"))?;
    let chat_id = numeric(chat.get("id"))?;
    let sender = message.get("from");
    let sender_id = sender.map(|sender| numeric(sender.get("id"))).transpose()?;
    if chat_id != payload.conversation.platform_id
        || sender_id
            .as_ref()
            .is_some_and(|id| *id != payload.sender.platform_id)
    {
        return Err(VerificationError("telegram_identity_mismatch"));
    }
    numeric(message.get("message_id"))?;
    if kind.starts_with("edited_") {
        return Ok(Observation::Refused(Refusal::MessageEdit));
    }
    if message.get("sender_chat").is_some() || *kind == "channel_post" {
        return Ok(Observation::Refused(Refusal::SystemSender));
    }
    match sender
        .and_then(|sender| sender.get("is_bot"))
        .and_then(Value::as_bool)
    {
        Some(true) => return Ok(Observation::Refused(Refusal::BotSender)),
        None => return Ok(Observation::Refused(Refusal::UnknownSender)),
        Some(false) => {}
    }
    if message.get("via_bot").is_some()
        || [
            "new_chat_members",
            "left_chat_member",
            "new_chat_title",
            "pinned_message",
        ]
        .iter()
        .any(|field| message.get(*field).is_some())
    {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    }
    if [
        "photo",
        "document",
        "video",
        "audio",
        "voice",
        "animation",
        "sticker",
        "video_note",
    ]
    .iter()
    .any(|field| message.get(*field).is_some())
        || !payload.content.attachments.is_empty()
    {
        return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
    }
    let text = message.get("text").and_then(Value::as_str);
    if [
        "contact",
        "location",
        "venue",
        "poll",
        "dice",
        "game",
        "invoice",
        "successful_payment",
    ]
    .iter()
    .any(|field| message.get(*field).is_some())
        || payload.content.kind != "text"
        || text.is_none()
        || payload.content.text.as_deref() != text
        || text.is_some_and(|text| text.is_empty() || text.len() > 32 * 1024)
    {
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
    let conversation = match chat.get("type").and_then(Value::as_str) {
        Some("private") => ConversationKind::Private,
        Some("group" | "supergroup") => ConversationKind::Group,
        _ => return Ok(Observation::Refused(Refusal::SystemEvent)),
    };
    // A relay route may cover many actual chats. Raw Telegram type is authoritative.
    let thread_id = match message.get("message_thread_id") {
        Some(id)
            if conversation == ConversationKind::Group
                && message.get("is_topic_message").and_then(Value::as_bool) == Some(true) =>
        {
            Some(numeric(Some(id))?)
        }
        _ => None,
    };
    let mentioned = bot_username
        .map(|username| mention(message, text.unwrap(), username))
        .transpose()?;
    let replied_to_bot = match message.get("reply_to_message") {
        None => Some(false),
        Some(parent) => parent
            .get("from")
            .and_then(|from| from.get("is_bot"))
            .and_then(Value::as_bool),
    };
    Ok(Observation::Message(TelegramFacts {
        callback: None,
        conversation,
        thread_id,
        mentioned,
        replied_to_bot,
    }))
}
fn numeric(value: Option<&Value>) -> Result<String, VerificationError> {
    value
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
        .ok_or(VerificationError("telegram_numeric_id"))
}
fn mention(message: &Value, text: &str, username: &str) -> Result<bool, VerificationError> {
    let Some(entities) = message.get("entities") else {
        return Ok(false);
    };
    let entities = entities
        .as_array()
        .ok_or(VerificationError("telegram_entities"))?;
    // Telegram entity offsets are UTF-16 code units, not UTF-8 bytes or Unicode scalar counts.
    let units: Vec<_> = text.encode_utf16().collect();
    for entity in entities {
        if entity.get("type").and_then(Value::as_str) != Some("mention") {
            continue;
        }
        let offset = entity
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(VerificationError("telegram_entities"))?;
        let length = entity
            .get("length")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or(VerificationError("telegram_entities"))?;
        let end = offset
            .checked_add(length)
            .ok_or(VerificationError("telegram_entities"))?;
        let slice = units
            .get(offset..end)
            .ok_or(VerificationError("telegram_entities"))?;
        let value =
            String::from_utf16(slice).map_err(|_| VerificationError("telegram_entities"))?;
        if value
            .strip_prefix('@')
            .is_some_and(|value| value.eq_ignore_ascii_case(username))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn observe_callback(
    payload: &CallbackPayload,
    callback: &Value,
) -> Result<Observation, VerificationError> {
    let denied = || VerificationError("telegram_callback_invalid");
    let from = callback.get("from").ok_or_else(denied)?;
    if from.get("is_bot").and_then(Value::as_bool) != Some(false) {
        return Ok(Observation::Refused(Refusal::BotSender));
    }
    let message = callback.get("message").ok_or_else(denied)?;
    let chat = message.get("chat").ok_or_else(denied)?;
    let message_id = numeric(message.get("message_id"))?;
    if numeric(from.get("id"))? != payload.sender.platform_id
        || numeric(chat.get("id"))? != payload.conversation.platform_id
        || payload.reply_to_platform_message_id.as_deref() != Some(message_id.as_str())
        || payload.content.kind != "interaction"
        || !payload.content.attachments.is_empty()
    {
        return Err(denied());
    }
    let conversation = match chat.get("type").and_then(Value::as_str) {
        Some("private") => ConversationKind::Private,
        Some("group" | "supergroup") => ConversationKind::Group,
        _ => return Err(denied()),
    };
    let id = callback
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 256)
        .ok_or_else(denied)?
        .to_owned();
    let data = callback
        .get("data")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .ok_or_else(denied)?
        .to_owned();
    let thread_id = message
        .get("message_thread_id")
        .map(|id| numeric(Some(id)))
        .transpose()?;
    Ok(Observation::Message(TelegramFacts {
        callback: Some(CallbackFacts {
            id,
            data,
            message_id,
        }),
        conversation,
        thread_id,
        mentioned: Some(true),
        replied_to_bot: Some(true),
    }))
}
