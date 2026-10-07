//! Lark and Feishu facts taken from the authenticated raw `im.message.receive_v1`
//! event and correlated to the normalized envelope.
use super::{
    facts::{Observation, RelayFacts},
    payload::CallbackPayload,
    VerificationError,
};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{ActorKind, ConversationKind};
use serde_json::Value;

const MESSAGE_EVENT: &str = "im.message.receive_v1";
const INVALID: VerificationError = VerificationError("lark_event_invalid");

/// Observe a Lark or Feishu message. `bot_id` is the source's pinned bot `open_id`.
///
/// Without a pinned `bot_id`, any mention counts as addressing the bot: Lark
/// delivers a group message to a bot only when it @mentions that bot, unless the
/// app holds the permission to read all group messages. Pin `bot_id` for exact
/// mention evidence when that permission is granted.
pub fn observe(
    payload: &CallbackPayload,
    bot_id: Option<&str>,
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
    match message.get("message_type").and_then(Value::as_str) {
        Some("text") => {}
        Some("image" | "file" | "audio" | "media" | "sticker") => {
            return Ok(Observation::Refused(Refusal::AttachmentUnsupported))
        }
        _ => return Ok(Observation::Refused(Refusal::ContentUnsupported)),
    }
    if payload.received_attachments() > 0 {
        return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
    }
    let text = message
        .get("content")
        .and_then(Value::as_str)
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .and_then(|content| {
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
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
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
    }))
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
