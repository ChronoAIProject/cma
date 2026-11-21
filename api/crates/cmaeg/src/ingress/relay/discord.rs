//! Opt-in Discord facts from signed NyxID callbacks, without platform I/O.
//! Interaction credentials are never projected as threads or saved as content.
use super::{
    facts::{Observation, RelayFacts},
    payload::CallbackPayload,
    verified_media::{self, numeric_id, Identity},
    VerificationError,
};
use crate::discord::facts::{bool_fact, mentions, reply_author};
use crate::ingress::admission::Refusal;
use cmaeg_core::activity::{
    ActorKind, Attachment, AttachmentKind, ConversationKind, MAX_ATTACHMENTS,
};
use serde_json::Value;

const INVALID: VerificationError = VerificationError("discord_event_invalid");
const IDENTITY: VerificationError = VerificationError("discord_identity_mismatch");

pub(super) fn observe(
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
    match raw.get("t").and_then(Value::as_str) {
        Some("MESSAGE_UPDATE") => return Ok(Observation::Refused(Refusal::MessageEdit)),
        Some("MESSAGE_CREATE") | None => {}
        Some(_) => return Ok(Observation::Refused(Refusal::SystemEvent)),
    }
    let interaction = match raw.get("type") {
        Some(value) if matches!(value.as_u64(), Some(2 | 3)) => true,
        // Native type 4 is autocomplete, not a message component.
        Some(_) => return Ok(Observation::Refused(Refusal::SystemEvent)),
        None => false,
    };
    let message = if interaction {
        raw
    } else {
        raw.get("d").unwrap_or(raw)
    };
    let message_id = numeric_id(&message["id"]).ok_or(INVALID)?;
    let chat_id = numeric_id(&message["channel_id"]).ok_or(INVALID)?;
    let user = if interaction {
        message
            .pointer("/member/user")
            .or_else(|| message.get("user"))
    } else {
        message.get("author")
    }
    .ok_or(INVALID)?;
    let sender = numeric_id(&user["id"]).ok_or(INVALID)?;
    let parent = if interaction {
        None
    } else {
        message["message_reference"]["message_id"].as_str()
    };
    if sender != payload.sender.platform_id
        || chat_id != payload.conversation.platform_id
        || parent != payload.reply_to_platform_message_id.as_deref()
        || message["channel"]["id"]
            .as_str()
            .is_some_and(|id| id != chat_id)
    {
        return Err(IDENTITY);
    }
    let is_bot =
        bool_fact(user, "bot")? || message.get("webhook_id").is_some() || bot_id == Some(sender);
    if is_bot {
        return Ok(Observation::Refused(Refusal::BotSender));
    }
    if bool_fact(user, "system")? {
        return Ok(Observation::Refused(Refusal::SystemSender));
    }
    if !interaction {
        if message
            .get("edited_timestamp")
            .is_some_and(|v| !v.is_null())
        {
            return Ok(Observation::Refused(Refusal::MessageEdit));
        }
        if message
            .get("type")
            .is_some_and(|v| !matches!(v.as_u64(), Some(0 | 19)))
        {
            return Ok(Observation::Refused(Refusal::SystemEvent));
        }
    }
    let conversation = if interaction && message["channel"]["type"] == 1 {
        ConversationKind::Private
    } else {
        // NyxID's Gateway-message adapter currently labels every message group.
        // Do not infer a private chat from an unauthenticated envelope hint.
        ConversationKind::Group
    };
    let expected_chat = if conversation == ConversationKind::Private {
        "private"
    } else {
        "group"
    };
    if payload.conversation.kind != expected_chat {
        return Err(IDENTITY);
    }
    let text = if interaction {
        interaction_text(message)
    } else {
        message["content"].as_str().map(str::to_owned)
    };
    if text != payload.content.text || text.as_ref().is_some_and(|v| v.len() > 32 * 1024) {
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
    let files = files(message, interaction)?;
    if !media_enabled && (!files.is_empty() || payload.received_attachments() > 0) {
        return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
    }
    let expected_type = if interaction {
        "text"
    } else {
        files.first().map_or("text", |file| kind(file).1)
    };
    if payload.content.kind != expected_type {
        return Ok(Observation::Refused(Refusal::ContentUnsupported));
    }
    if files.is_empty() {
        if payload.received_attachments() > 0 {
            return Ok(Observation::Refused(Refusal::AttachmentUnsupported));
        }
        if text.as_ref().is_none_or(String::is_empty) {
            return Ok(Observation::Refused(Refusal::ContentUnsupported));
        }
    }
    let attachments: Vec<Attachment> = files
        .into_iter()
        .take(MAX_ATTACHMENTS)
        .enumerate()
        .map(|(index, file)| {
            verified_media::extract(
                Identity {
                    platform: "discord",
                    message_id,
                    file_id: numeric_id(&file["id"]),
                    url: file["url"].as_str(),
                },
                file,
                kind(file).0,
                &payload.content.attachments,
                index,
            )
        })
        .collect();
    let mentioned = if interaction {
        if let Some(pinned) = bot_id {
            if numeric_id(&message["application_id"]) != Some(pinned) {
                return Err(IDENTITY);
            }
            Some(true)
        } else {
            None
        }
    } else {
        mentions(message, bot_id)?
    };
    let replied_to_bot = if let Some(parent) = parent {
        reply_author(message, parent, chat_id, bot_id)?
    } else {
        Some(false)
    };
    let channel_type = message["channel_type"]
        .as_u64()
        .or_else(|| message["channel"]["type"].as_u64());
    Ok(Observation::Message(RelayFacts {
        thread: None,
        callback: None,
        event: None,
        conversation,
        // Only explicit membership, never an embedded child thread or an
        // interaction follow-up token, supplies a partition key.
        thread_id: (!interaction && matches!(channel_type, Some(10..=12)))
            .then(|| chat_id.to_owned()),
        mentioned,
        replied_to_bot,
        actor: ActorKind::Human,
        text,
        attachments,
        album_id: None,
    }))
}

fn kind(file: &Value) -> (AttachmentKind, &'static str) {
    let mime = file["content_type"].as_str().unwrap_or("");
    if mime.starts_with("image/") {
        (AttachmentKind::Image, "image")
    } else if mime.starts_with("audio/") {
        (AttachmentKind::Audio, "audio")
    } else if mime.starts_with("video/") {
        (AttachmentKind::Video, "video")
    } else {
        (AttachmentKind::File, "file")
    }
}

fn files(message: &Value, interaction: bool) -> Result<Vec<&Value>, VerificationError> {
    let entries: Vec<&Value> = if let Some(value) = message.get("attachments") {
        value.as_array().ok_or(INVALID)?.iter().collect()
    } else if interaction {
        match message.pointer("/data/resolved/attachments") {
            Some(Value::Object(files)) => files.values().collect(),
            None => Vec::new(),
            _ => return Err(INVALID),
        }
    } else {
        Vec::new()
    };
    Ok(entries
        .into_iter()
        .filter(|file| file["url"].is_string())
        .collect())
}

/// NyxID's slash-command flattening; components stay ordinary content until
/// the gateway has issued and verified its own interaction capability.
fn interaction_text(message: &Value) -> Option<String> {
    let data = message.get("data")?;
    if let Some(name) = data["name"].as_str() {
        let options = data["options"]
            .as_array()
            .map(|options| {
                options
                    .iter()
                    .filter_map(|option| {
                        let name = option["name"].as_str()?;
                        let value = option.get("value")?;
                        let value = value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string());
                        Some(format!("{name}={value}"))
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        return Some(if options.is_empty() {
            format!("/{name}")
        } else {
            format!("/{name} {options}")
        });
    }
    data["custom_id"].as_str().map(str::to_owned)
}
