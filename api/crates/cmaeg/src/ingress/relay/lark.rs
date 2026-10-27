//! Lark and Feishu facts taken from the authenticated raw `im.message.receive_v1`
//! event and correlated to the normalized envelope.
use super::{
    facts::{Observation, RelayFacts},
    lark_media, lark_post,
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
/// carry a mention, so a group under `mention` admission refuses them; a post
/// that @mentions the bot is addressed like a text message.
pub fn observe(
    payload: &CallbackPayload,
    bot_id: Option<&str>,
    allow_bots: bool,
    media_enabled: bool,
) -> Result<Observation, VerificationError> {
    let Some(raw) = payload
        .raw_platform_data
        .as_ref()
        .filter(|raw| raw.is_object())
    else {
        return Ok(Observation::Refused(Refusal::UnknownSender));
    };
    if raw.pointer("/header/event_type").and_then(Value::as_str) == Some("card.action.trigger") {
        return super::lark_callback::observe(payload, raw);
    }
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
    let actor = match sender.get("sender_type").and_then(Value::as_str) {
        Some("user") => ActorKind::Human,
        Some("app") if allow_bots => ActorKind::Bot,
        Some("app") => return Ok(Observation::Refused(Refusal::BotSender)),
        _ => return Ok(Observation::Refused(Refusal::UnknownSender)),
    };
    // A human sender is always bound to the open_id NyxID reports for it.
    if open_id.is_none()
        || (actor == ActorKind::Bot && !open_id.is_some_and(|id| id.starts_with("ou_")))
    {
        return Err(VerificationError("lark_identity_mismatch"));
    }
    // A post resolves its mentions while it is flattened, but a malformed list
    // is still answered only after the content and chat type, as before posts.
    let mentions = match message.get("mentions") {
        None | Some(Value::Null) => Ok(&[][..]),
        Some(Value::Array(mentions)) => Ok(mentions.as_slice()),
        Some(_) => Err(INVALID),
    };
    let listed = mentions.unwrap_or_default();
    let content = match content(payload, message, listed, bot_id, media_enabled) {
        Ok(content) => content,
        Err(refusal) => return Ok(Observation::Refused(refusal)),
    };
    let conversation = match message.get("chat_type").and_then(Value::as_str) {
        Some("p2p") => ConversationKind::Private,
        Some("group") => ConversationKind::Group,
        _ => return Ok(Observation::Refused(Refusal::SystemEvent)),
    };
    let mentions = mentions?;
    let mentioned = if bot_id.is_some() {
        Some(crate::ingress::native_mentions::lark(message, bot_id).bot)
    } else if actor == ActorKind::Human {
        // Preserve legacy human-only sources. Bot opt-in validation requires
        // a pinned open_id and can never use this compatibility path.
        Some(!mentions.is_empty())
    } else {
        None
    };
    // The parent's author is not in the event, so a reply is not evidence either way.
    let replied_to_bot = match identifier(message.get("parent_id"))? {
        Some(_) => None,
        None => Some(false),
    };
    Ok(Observation::Message(RelayFacts {
        thread: None,
        event: None,
        callback: None,
        conversation,
        thread_id: identifier(message.get("thread_id"))?.map(str::to_owned),
        mentioned,
        replied_to_bot,
        actor,
        text: content.text,
        attachments: content.attachments,
        album_id: None,
    }))
}

/// What a message's content contributes beyond NyxID's envelope.
#[derive(Default)]
struct Content {
    /// Text the observer derived from the raw event, overriding `content.text`.
    text: Option<String>,
    attachments: Vec<Attachment>,
}

/// The content of a message, or the refusal it earns. With media off, this is
/// exactly the text-only rule that predates media.
fn content(
    payload: &CallbackPayload,
    message: &Value,
    mentions: &[Value],
    bot_id: Option<&str>,
    media_enabled: bool,
) -> Result<Content, Refusal> {
    match message.get("message_type").and_then(Value::as_str) {
        Some("text") => text(payload, message).map(|()| Content::default()),
        Some(message_type @ ("image" | "file" | "audio" | "media" | "video")) if media_enabled => {
            let attachments = media(payload, message, message_type)?;
            Ok(Content {
                text: None,
                attachments,
            })
        }
        Some("post") if media_enabled => post(payload, message, mentions, bot_id),
        // NyxID offers no download for a sticker, so it stays refused with media on.
        Some("image" | "file" | "audio" | "media" | "video" | "sticker") => {
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

/// An image, file, audio, media or video message. NyxID lists the message's one
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

/// A rich-text post. NyxID relays it as `content.type:"unknown"` without text
/// or attachments, so its text is flattened from the raw event, which the
/// callback JWT binds through `body_sha256`.
fn post(
    payload: &CallbackPayload,
    message: &Value,
    mentions: &[Value],
    bot_id: Option<&str>,
) -> Result<Content, Refusal> {
    if payload.content.kind != "unknown"
        || payload.content.text.is_some()
        || payload.received_attachments() > 0
    {
        return Err(Refusal::ContentUnsupported);
    }
    let post = raw_content(message)
        .and_then(|content| lark_post::flatten(&content, mentions, bot_id))
        .ok_or(Refusal::ContentUnsupported)?;
    // Sizes only: the text and keys are sender data.
    tracing::debug!(
        bytes = post.text.len(),
        placeholders = post.placeholders.len(),
        "lark post flattened"
    );
    Ok(Content {
        text: Some(post.text),
        attachments: post.placeholders,
    })
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

/// Called only after `observe` authenticated and correlated the raw message.
/// Missing ancestry remains unresolved; a parent is not assumed to be a root.
pub(crate) fn thread_facts(
    payload: &CallbackPayload,
    bot_id: &str,
) -> Result<Option<cmaeg_core::thread::ThreadFacts>, VerificationError> {
    use cmaeg_core::thread::{ThreadAddress, ThreadFacts, ThreadKind};
    let message = payload
        .raw_platform_data
        .as_ref()
        .and_then(|raw| raw.pointer("/event/message"))
        .ok_or(INVALID)?;
    if message["chat_type"] != "group"
        || payload
            .raw_platform_data
            .as_ref()
            .and_then(|raw| raw.pointer("/event/sender/sender_type"))
            .and_then(Value::as_str)
            != Some("user")
    {
        return Ok(None);
    }
    let message_id = identifier(message.get("message_id"))?
        .ok_or(INVALID)?
        .to_owned();
    let parent = identifier(message.get("parent_id"))?.map(str::to_owned);
    let native = identifier(message.get("thread_id"))?.map(str::to_owned);
    let root = identifier(message.get("root_id"))?
        .map(str::to_owned)
        .or_else(|| (parent.is_none() && native.is_none()).then(|| message_id.clone()));
    let mentions = crate::ingress::native_mentions::lark(message, Some(bot_id));
    Ok(Some(ThreadFacts {
        version: 1,
        kind: ThreadKind::Native,
        chat_id: payload.conversation.platform_id.clone(),
        message_id,
        root_id: root,
        native_thread_id: native,
        parent_message_id: parent,
        sender_kind: ActorKind::Human,
        address: if mentions.bot {
            ThreadAddress::Mention
        } else {
            ThreadAddress::NotAddressed
        },
        mentions_others: mentions.other && !mentions.bot && !mentions.everyone,
    }))
}
