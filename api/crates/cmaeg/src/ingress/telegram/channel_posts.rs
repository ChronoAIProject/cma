//! Native channel posts retain a channel identity; no human author is inferred.
use super::{Observed, Outcome};
use crate::ingress::{
    admission::{admit, AdmissionFacts, Refusal},
    relay::{
        facts::RelayFacts,
        payload::CallbackPayload,
        telegram_media::{extract, Transport, MEDIA_FIELDS},
        VerificationError,
    },
};
use cmaeg_core::{
    activity::{ActorKind, ConversationKind},
    channel::{AdmissionPolicy, IdentityAdmission},
};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Sender allowlists describe humans, not the undisclosed publisher of a channel.
/// Chat allowlists and non-private addressing still run through ordinary admission.
pub(crate) fn admit_channel(
    policy: &AdmissionPolicy,
    facts: &AdmissionFacts<'_>,
) -> Result<(), Refusal> {
    match policy {
        AdmissionPolicy::All {}
        | AdmissionPolicy::Scoped {
            senders: IdentityAdmission::Open,
            ..
        } => {}
        _ => return Err(Refusal::UnknownSender),
    }
    admit(policy, facts)
}

pub(super) fn observe(
    raw: Value,
    bot_id: &str,
    bot_username: Option<&str>,
    media_enabled: bool,
) -> Result<Outcome, VerificationError> {
    let invalid = || VerificationError("telegram_channel_post_invalid");
    if raw.as_object().is_none_or(|object| object.len() != 2) {
        return Err(invalid());
    }
    let update_id = raw["update_id"]
        .as_i64()
        .filter(|id| *id >= 0)
        .ok_or_else(invalid)?;
    let message = &raw["channel_post"];
    let chat = &message["chat"];
    let chat_id = chat["id"]
        .as_i64()
        .filter(|id| *id < 0)
        .ok_or_else(invalid)?;
    if chat["type"] != "channel" || chat.get("is_direct_messages").is_some() {
        return Err(invalid());
    }
    if let Some(sender) = message.get("sender_chat") {
        if sender["id"].as_i64() != Some(chat_id) || sender["type"] != "channel" {
            return Err(invalid());
        }
    }
    // Channel posts have no forum/private topic. Discussion-group forwards and
    // business/direct-message updates belong to different admission contracts.
    if [
        "message_thread_id",
        "is_topic_message",
        "direct_messages_topic",
        "business_connection_id",
        "guest_query_id",
        "sender_business_bot",
        "via_bot",
        "is_automatic_forward",
        "edit_date",
    ]
    .iter()
    .any(|key| message.get(*key).is_some())
    {
        return Ok(Outcome::Refused(Refusal::SystemEvent));
    }
    if [
        "new_chat_members",
        "left_chat_member",
        "new_chat_title",
        "pinned_message",
        "managed_bot_created",
    ]
    .iter()
    .any(|key| message.get(*key).is_some())
    {
        return Ok(Outcome::Refused(Refusal::SystemEvent));
    }
    let message_id = message["message_id"]
        .as_i64()
        .filter(|id| *id > 0)
        .ok_or_else(invalid)?
        .to_string();
    let date = message["date"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(invalid)?;
    let timestamp = OffsetDateTime::from_unix_timestamp(date)
        .map_err(|_| invalid())?
        .format(&Rfc3339)
        .map_err(|_| invalid())?;
    let has_media = MEDIA_FIELDS.iter().any(|key| message.get(*key).is_some());
    if has_media && !media_enabled {
        return Ok(Outcome::Refused(Refusal::AttachmentUnsupported));
    }
    let media = has_media
        .then(|| extract(message, Transport::Native, "telegram"))
        .flatten();
    if has_media
        && media
            .as_ref()
            .is_none_or(|media| media.attachments.is_empty())
    {
        return Ok(Outcome::Refused(Refusal::AttachmentUnsupported));
    }
    let shared = match super::shared_facts::extract(message) {
        Ok(shared) => shared,
        Err(()) => return Ok(Outcome::Refused(Refusal::ContentUnsupported)),
    };
    let text = match &media {
        Some(media) => media.caption.clone(),
        None => shared.or_else(|| message["text"].as_str().map(str::to_owned)),
    };
    if ["poll", "dice", "game", "invoice", "successful_payment"]
        .iter()
        .any(|key| message.get(*key).is_some())
        || (has_media && message.get("text").is_some())
        || (!has_media && text.as_ref().is_none_or(String::is_empty))
        || text.as_ref().is_some_and(|text| text.len() > 32 * 1024)
        || message
            .get("caption")
            .is_some_and(|caption| !caption.is_string())
    {
        return Ok(Outcome::Refused(Refusal::ContentUnsupported));
    }
    let mentions = crate::ingress::native_mentions::telegram(message, Some(bot_id), bot_username);
    if mentions.invalid {
        return Err(VerificationError("telegram_entities"));
    }
    let (attachments, album_id) = media
        .as_ref()
        .map(|m| (m.attachments.clone(), m.album_id.clone()))
        .unwrap_or_default();
    let chat_id = chat_id.to_string();
    let mut payload: CallbackPayload = serde_json::from_value(json!({
        "message_id":format!("telegram:{bot_id}:{update_id}"), "correlation_id":update_id.to_string(),
        "platform":"telegram", "reply_token":null, "agent":{"api_key_id":bot_id,"name":""},
        "conversation":{"id":crate::telegram::route(bot_id,&chat_id),"platform_id":chat_id,"type":"channel"},
        "sender":{"platform_id":format!("telegram:chat:{chat_id}"),"display_name":chat.get("title")},
        "content":{"type":media.as_ref().map_or("text",|m|m.nyxid_type),"text":text},
        "reply_to_platform_message_id":null,"timestamp":timestamp,"raw_platform_data":raw,
    })).map_err(|_| invalid())?;
    payload.sender.repair_display_name();
    payload.validate()?;
    Ok(Outcome::Admitted(Box::new(Observed {
        payload,
        update_id,
        message_id,
        facts: RelayFacts {
            thread: None,
            event: None,
            callback: None,
            conversation: ConversationKind::Channel,
            thread_id: None,
            mentioned: Some(mentions.bot),
            replied_to_bot: Some(false),
            actor: ActorKind::Unknown,
            text: None,
            attachments,
            album_id,
        },
    })))
}
