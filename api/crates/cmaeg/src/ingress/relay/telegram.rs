//! Telegram facts taken from authenticated raw fields and correlated to the envelope.
use super::{
    facts::{CallbackFacts, Observation, RelayFacts},
    payload::CallbackPayload,
    telegram_media::{extract, MediaIngress, TelegramMedia, Transport, MEDIA_FIELDS},
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, ConversationKind};
use serde_json::Value;

/// Longest message text or media caption admitted, matching the activity bound.
const MAX_TEXT_BYTES: usize = 32 * 1024;

pub fn observe(
    payload: &CallbackPayload,
    bot_username: Option<&str>,
    bot_id: Option<&str>,
    allow_bots: bool,
    ingress: MediaIngress,
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
    let actor = match sender
        .and_then(|sender| sender.get("is_bot"))
        .and_then(Value::as_bool)
    {
        Some(true) if allow_bots => ActorKind::Bot,
        Some(true) => return Ok(Observation::Refused(Refusal::BotSender)),
        None => return Ok(Observation::Refused(Refusal::UnknownSender)),
        Some(false) => ActorKind::Human,
    };
    if actor == ActorKind::Bot
        && !sender_id.as_deref().is_some_and(|id| {
            id.parse::<i64>()
                .is_ok_and(|n| n > 0 && n.to_string() == id)
        })
    {
        return Ok(Observation::Refused(Refusal::UnknownSender));
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
    let media = match media(payload, message, ingress) {
        Ok(media) => media,
        Err(reason) => return Ok(Observation::Refused(reason)),
    };
    if message.get("managed_bot_created").is_some() {
        if actor != ActorKind::Human {
            return Ok(Observation::Refused(Refusal::BotSender));
        }
        return super::telegram_managed::observe(payload, raw, message);
    }
    let text = message.get("text").and_then(Value::as_str);
    // NyxID's content type and text must describe the raw message: text for a
    // plain message, NyxID's media type and the caption for a media message.
    let content_agrees = match &media {
        None => {
            payload.content.kind == "text"
                && payload.content.text.as_deref() == text
                && text.is_some_and(|text| !text.is_empty() && text.len() <= MAX_TEXT_BYTES)
        }
        Some(media) => {
            payload.content.kind == media.nyxid_type
                && payload.content.text.as_deref() == media.caption.as_deref()
                && media
                    .caption
                    .as_ref()
                    .is_none_or(|caption| caption.len() <= MAX_TEXT_BYTES)
        }
    };
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
        || !content_agrees
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
    let native = crate::ingress::native_mentions::telegram(message, bot_id, bot_username);
    if native.invalid {
        return Err(VerificationError("telegram_entities"));
    }
    let mentioned = (bot_id.is_some() || bot_username.is_some()).then_some(native.bot);
    let replied_to_bot = match message.get("reply_to_message") {
        None => Some(false),
        Some(parent) => parent
            .get("from")
            .and_then(|from| from.get("is_bot"))
            .and_then(Value::as_bool),
    };
    let (attachments, album_id) = media
        .map(|media| (media.attachments, media.album_id))
        .unwrap_or_default();
    Ok(Observation::Message(RelayFacts {
        thread: None,
        event: None,
        callback: None,
        conversation,
        thread_id,
        mentioned,
        replied_to_bot,
        actor,
        text: None,
        attachments,
        album_id,
    }))
}

/// The message's media under `ingress`, or the refusal it earns. Off, any media
/// field or envelope attachment is refused exactly as before media existed.
fn media(
    payload: &CallbackPayload,
    message: &Value,
    ingress: MediaIngress,
) -> Result<Option<TelegramMedia>, Refusal> {
    let (transport, source) = match ingress {
        MediaIngress::Off => {
            let present = MEDIA_FIELDS
                .iter()
                .any(|field| message.get(*field).is_some());
            return if present || payload.received_attachments() > 0 {
                Err(Refusal::AttachmentUnsupported)
            } else {
                Ok(None)
            };
        }
        MediaIngress::Relay => (
            Transport::Relay(&payload.content.attachments),
            "nyxid_relay",
        ),
        MediaIngress::Native => (Transport::Native, "telegram"),
    };
    match extract(message, transport, source) {
        // The envelope lists attachments that the raw message does not carry.
        None if payload.received_attachments() > 0 => Err(Refusal::AttachmentUnsupported),
        None => Ok(None),
        Some(media) if media.attachments.is_empty() => Err(Refusal::AttachmentUnsupported),
        Some(media) => Ok(Some(media)),
    }
}
fn numeric(value: Option<&Value>) -> Result<String, VerificationError> {
    value
        .and_then(Value::as_i64)
        .map(|id| id.to_string())
        .ok_or(VerificationError("telegram_numeric_id"))
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
        || payload.received_attachments() > 0
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
    Ok(Observation::Message(RelayFacts {
        thread: None,
        event: None,
        callback: Some(CallbackFacts {
            id,
            data,
            message_id,
        }),
        conversation,
        thread_id,
        mentioned: Some(true),
        replied_to_bot: Some(true),
        actor: ActorKind::Human,
        text: None,
        attachments: Vec::new(),
        album_id: None,
    }))
}
