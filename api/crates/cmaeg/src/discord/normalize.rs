//! Native Gateway facts; no relay envelope or relay credential is synthesized.
use super::{facts, GatewayError};
use crate::ingress::admission::{admit, AdmissionFacts, Refusal};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, Actor, ActorKind, Attachment, AttachmentKind,
        AttachmentSource, ConversationKind, SourceKind, MAX_ATTACHMENTS,
    },
    channel::{valid_discord_id as valid_id, AdmissionPolicy, GroupAdmission},
    ids::EventId,
};
use serde_json::Value;

pub(super) enum Normalized {
    Activity(Box<Activity>),
    Refused(Refusal),
}
pub(super) fn normalize(
    raw: &Value,
    bot: &str,
    policy: &AdmissionPolicy,
) -> Result<Normalized, GatewayError> {
    normalize_with_content(raw, bot, policy, None)
}
pub(super) fn normalize_with_content(
    raw: &Value,
    bot: &str,
    policy: &AdmissionPolicy,
    content: Option<&super::content::Snapshot>,
) -> Result<Normalized, GatewayError> {
    normalize_with_projections(raw, bot, policy, content, None)
}
pub(super) fn normalize_with_projections(
    raw: &Value,
    bot: &str,
    policy: &AdmissionPolicy,
    content: Option<&super::content::Snapshot>,
    forward: Option<&super::forward::Snapshot>,
) -> Result<Normalized, GatewayError> {
    let id = snowflake(&raw["id"])?;
    let chat = snowflake(&raw["channel_id"])?;
    let sender = snowflake(&raw["author"]["id"])?;
    let boolean = |value, name| facts::bool_fact(value, name).map_err(|_| GatewayError::Protocol);
    let is_bot =
        boolean(&raw["author"], "bot")? || raw.get("webhook_id").is_some() || sender == bot;
    if is_bot {
        return Ok(Normalized::Refused(Refusal::BotSender));
    }
    if boolean(&raw["author"], "system")? {
        return Ok(Normalized::Refused(Refusal::SystemSender));
    }
    if raw.get("edited_timestamp").is_some_and(|v| !v.is_null()) {
        return Ok(Normalized::Refused(Refusal::MessageEdit));
    }
    if !matches!(raw["type"].as_u64(), Some(0 | 19)) {
        return Ok(Normalized::Refused(Refusal::SystemEvent));
    }
    let group = match raw.get("guild_id") {
        Some(value) => {
            snowflake(value)?;
            true
        }
        None => raw["channel_type"].as_u64() == Some(3),
    };
    if let Some(value) = raw.pointer("/channel/id") {
        if snowflake(value)? != chat {
            return Err(GatewayError::Protocol);
        }
    }
    let parent = match raw.get("message_reference") {
        Some(reference) => Some(snowflake(&reference["message_id"])?),
        None => None,
    };
    if let Some(snapshot) = forward {
        snapshot.verify(raw)?;
    }
    let reply = match parent {
        Some(_) if forward.is_some() => Ok(Some(false)),
        Some(parent) => facts::reply_author(raw, parent, chat, Some(bot)),
        None => Ok(Some(false)),
    }
    .map_err(|_| GatewayError::Protocol)?;
    // Malformed mention identities are refused, never interpreted as absence.
    if let Some(mentions) = raw.get("mentions") {
        for mention in mentions.as_array().ok_or(GatewayError::Protocol)? {
            snowflake(&mention["id"])?;
        }
    }
    let mentioned = facts::mentions(raw, Some(bot)).map_err(|_| GatewayError::Protocol)?;
    let addressed = match policy {
        AdmissionPolicy::Scoped {
            groups: GroupAdmission::Mention,
            ..
        } => mentioned,
        AdmissionPolicy::Scoped {
            groups: GroupAdmission::ReplyToBot,
            ..
        } => reply,
        _ => match (mentioned, reply) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
    };
    if let Err(reason) = admit(
        policy,
        &AdmissionFacts {
            receiving_bot_id: Some(bot),
            mentioned,
            sender_id: sender,
            chat_id: chat,
            is_group: group,
            is_bot_sender: false,
            is_system: false,
            addressed,
        },
    ) {
        return Ok(Normalized::Refused(reason));
    }
    let text = raw["content"].as_str().ok_or(GatewayError::Protocol)?;
    if text.len() > 32 * 1024 {
        return Ok(Normalized::Refused(Refusal::ContentUnsupported));
    }
    let text = match content {
        Some(snapshot) => snapshot.text(raw)?,
        None => text.to_owned(),
    };
    let text = match forward {
        Some(snapshot) => snapshot.append(raw, text)?,
        None => text,
    };
    let files = raw["attachments"]
        .as_array()
        .ok_or(GatewayError::Protocol)?;
    if files.len() > MAX_ATTACHMENTS {
        return Ok(Normalized::Refused(Refusal::AttachmentUnsupported));
    }
    if text.is_empty() && files.is_empty() {
        return Ok(Normalized::Refused(Refusal::ContentUnsupported));
    }
    let mut attachments = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for file in files {
        let file_id = snowflake(&file["id"])?;
        if !seen.insert(file_id) {
            return Err(GatewayError::Protocol);
        }
        let mime = file["content_type"].as_str();
        let kind = match mime.unwrap_or("").split('/').next() {
            Some("image") => AttachmentKind::Image,
            Some("audio") => AttachmentKind::Audio,
            Some("video") => AttachmentKind::Video,
            _ => AttachmentKind::File,
        };
        let mut attachment = Attachment {
            reference: format!("discord:{id}:{file_id}"),
            kind,
            filename: file["filename"].as_str().map(str::to_owned),
            mime_type: mime.map(str::to_owned),
            size_bytes: file["size"].as_u64(),
            source: Some(AttachmentSource::Discord {
                channel_id: chat.into(),
                message_id: id.into(),
                attachment_id: file_id.into(),
            }),
            variant: None,
            duration_seconds: None,
            width: None,
            height: None,
            thumbnail: None,
            emoji: None,
        };
        attachment.normalize_facts();
        attachments.push(attachment);
    }
    let activity = Activity {
        thread: None,
        schema_version: 1,
        event_id: EventId::new(format!("discord:{bot}:{id}"))
            .map_err(|_| GatewayError::Protocol)?,
        kind: ActivityKind::Message {
            text,
            attachments,
            mentions_bot: mentioned == Some(true),
            reply_to: None,
            quoted_message: None,
            album_id: None,
        },
        actor: Actor {
            id: sender.into(),
            display_name: raw["author"]["global_name"]
                .as_str()
                .or_else(|| raw["author"]["username"].as_str())
                .map(str::to_owned),
            kind: ActorKind::Human,
        },
        conversation: ActivityConversation {
            id: chat.into(),
            kind: if group {
                ConversationKind::Group
            } else {
                ConversationKind::Private
            },
            thread_id: None,
        },
        occurred_at: raw["timestamp"]
            .as_str()
            .ok_or(GatewayError::Protocol)?
            .into(),
        source: SourceKind::Discord { bot_id: bot.into() },
        routing: None,
    };
    activity.validate().map_err(|_| GatewayError::Protocol)?;
    Ok(Normalized::Activity(Box::new(activity)))
}
pub(super) fn snowflake(value: &Value) -> Result<&str, GatewayError> {
    value
        .as_str()
        .filter(|id| valid_id(id))
        .ok_or(GatewayError::Protocol)
}
