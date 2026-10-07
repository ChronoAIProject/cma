//! Lark and Feishu facts taken from the authenticated raw `im.message.receive_v1`
//! event and correlated to the normalized envelope.
use super::{
    facts::{Observation, RelayFacts},
    lark_media,
    payload::CallbackPayload,
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, Attachment, ConversationKind};
use serde_json::Value;

const MESSAGE_EVENT: &str = "im.message.receive_v1";
const INVALID: VerificationError = VerificationError("lark_event_invalid");

/// Observe a Lark or Feishu message. `bot_id` is the source's pinned bot `open_id`;
/// `media_enabled` is the operator's inbound media switch.
///
/// Without a pinned `bot_id`, any mention counts as addressing the bot: Lark
/// delivers a group message to a bot only when it @mentions that bot, unless the
/// app holds the permission to read all group messages. Pin `bot_id` for exact
/// mention evidence when that permission is granted. Lark media messages cannot
/// carry a mention, so a group under `mention` admission refuses them.
pub fn observe(
    payload: &CallbackPayload,
    bot_id: Option<&str>,
    media_enabled: bool,
) -> Result<Observation, VerificationError> {
    let Some(raw) = payload
        .raw_platform_data
        .as_ref()
        .filter(|raw| raw.is_object())
    else {
        return Ok(Observation::Refused(Refusal::UnknownSender));
    };
    if raw.pointer("/header/event_type").and_then(Value::as_str) != Some(MESSAGE_EVENT) {
        return Ok(Observation::Refused(Refusal::SystemEvent));
    }
    let message = raw.pointer("/event/message").ok_or(INVALID)?;
    let sender = raw.pointer("/event/sender").ok_or(INVALID)?;
    let chat_id = identifier(message.get("chat_id"))?.ok_or(INVALID)?;
    let open_id = identifier(sender.pointer("/sender_id/open_id"))?;
    if chat_id != payload.conversation.platform_id
        || open_id.is_some_and(|id| id != payload.sender.platform_id)
    {
        return Err(VerificationError("lark_identity_mismatch"));
    }
    identifier(message.get("message_id"))?.ok_or(INVALID)?;
    match sender.get("sender_type").and_then(Value::as_str) {
        Some("user") => {}
        Some("app") => return Ok(Observation::Refused(Refusal::BotSender)),
        _ => return Ok(Observation::Refused(Refusal::UnknownSender)),
    }
    // A human sender is always bound to the open_id NyxID reports for it.
    if open_id.is_none() {
        return Err(VerificationError("lark_identity_mismatch"));
    }
    let attachments = match content(payload, message, media_enabled) {
        Ok(attachments) => attachments,
        Err(refusal) => return Ok(Observation::Refused(refusal)),
    };
    let conversation = match message.get("chat_type").and_then(Value::as_str) {
        Some("p2p") => ConversationKind::Private,
        Some("group") => ConversationKind::Group,
        _ => return Ok(Observation::Refused(Refusal::SystemEvent)),
    };
    let mentions = match message.get("mentions") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(mentions)) => mentions.as_slice(),
        Some(_) => return Err(INVALID),
    };
    let mentioned = match bot_id {
        Some(bot_id) => mentions
            .iter()
            .any(|mention| mention.pointer("/id/open_id").and_then(Value::as_str) == Some(bot_id)),
        None => !mentions.is_empty(),
    };
    // The parent's author is not in the event, so a reply is not evidence either way.
    let replied_to_bot = match identifier(message.get("parent_id"))? {
        Some(_) => None,
        None => Some(false),
    };
    Ok(Observation::Message(RelayFacts {
        event: None,
        callback: None,
        conversation,
        thread_id: identifier(message.get("thread_id"))?.map(str::to_owned),
        mentioned: Some(mentioned),
        replied_to_bot,
        actor: ActorKind::Human,
        attachments,
        album_id: None,
    }))
}

/// The attachments a message's content contributes, or the refusal it earns.
/// With media off, this is exactly the text-only rule that predates media.
fn content(
    payload: &CallbackPayload,
    message: &Value,
    media_enabled: bool,
) -> Result<Vec<Attachment>, Refusal> {
    match message.get("message_type").and_then(Value::as_str) {
        Some("text") => text(payload, message).map(|()| Vec::new()),
        Some(message_type @ ("image" | "file" | "audio" | "media")) if media_enabled => {
            media(payload, message, message_type)
        }
        // NyxID offers no download for a sticker, so it stays refused with media on.
        Some("image" | "file" | "audio" | "media" | "sticker") => {
            Err(Refusal::AttachmentUnsupported)
        }
        _ => Err(Refusal::ContentUnsupported),
    }
}

/// A text message: its content JSON `text` must equal NyxID's `content.text`.
fn text(payload: &CallbackPayload, message: &Value) -> Result<(), Refusal> {
    if payload.received_attachments() > 0 {
        return Err(Refusal::AttachmentUnsupported);
    }
    let text = raw_content(message).and_then(|content| {
        content
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if payload.content.kind != "text"
        || text.is_none()
        || payload.content.text != text
        || text
            .as_deref()
            .is_some_and(|text| text.is_empty() || text.len() > 32 * 1024)
    {
        return Err(Refusal::ContentUnsupported);
    }
    Ok(())
}

/// An image, file, audio or media message. NyxID lists the message's one
/// resource as an envelope attachment, so the presence refusal for text does
/// not apply; its `content.type` must agree and it carries no text.
fn media(
    payload: &CallbackPayload,
    message: &Value,
    message_type: &str,
) -> Result<Vec<Attachment>, Refusal> {
    let content = raw_content(message)
        .filter(Value::is_object)
        .ok_or(Refusal::ContentUnsupported)?;
    let media = lark_media::extract(message_type, &content, &payload.content.attachments)
        .ok_or(Refusal::ContentUnsupported)?;
    if payload.content.kind != media.nyxid_type || payload.content.text.is_some() {
        return Err(Refusal::ContentUnsupported);
    }
    // Field names only: keys, names and durations are sender data.
    tracing::debug!(
        message_type,
        located = media.attachment.source.is_some(),
        repaired = ?media.repairs.fields().collect::<Vec<_>>(),
        "lark media extracted"
    );
    Ok(vec![media.attachment])
}

/// `event.message.content`: Lark sends it as a string holding JSON.
fn raw_content(message: &Value) -> Option<Value> {
    message
        .get("content")
        .and_then(Value::as_str)
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
}

/// An optional bounded identifier; absent, null and empty values are absent.
fn identifier(value: Option<&Value>) -> Result<Option<&str>, VerificationError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if id.is_empty() => Ok(None),
        Some(Value::String(id)) if id.len() <= 512 && !id.chars().any(char::is_control) => {
            Ok(Some(id))
        }
        Some(_) => Err(INVALID),
    }
}
