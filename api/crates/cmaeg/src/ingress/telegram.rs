//! Native Telegram update normalization reuses the strict platform observer.
use super::{
    launch::LaunchRequest,
    media_guard::{self, Admit, Built},
    relay::{
        facts::{Observation, RelayFacts},
        payload::CallbackPayload,
        telegram_media::{media_content_type, MediaIngress},
        VerificationError,
    },
};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{Activity, ActivityConversation, ActivityKind, Actor, SourceKind},
    ids::{ConversationId, EventId, IdempotencyKey},
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) mod channel_posts;
pub(crate) mod events;
pub(crate) mod member_joins;
mod message_context;
pub(crate) mod shared_facts;

pub struct Observed {
    pub payload: CallbackPayload,
    pub facts: RelayFacts,
    pub update_id: i64,
    pub message_id: String,
}
pub enum Outcome {
    Admitted(Box<Observed>),
    Refused(super::admission::Refusal),
}
/// The channel-post extension is native-only and requires an explicit source opt-in.
pub(crate) fn observe_source(
    raw: Value,
    bot_id: &str,
    bot_username: Option<&str>,
    media_enabled: bool,
    admission: &cmaeg_core::channel::AdmissionPolicy,
    channel_posts: bool,
) -> Result<Outcome, VerificationError> {
    if raw.get("channel_post").is_some() {
        return if channel_posts {
            self::channel_posts::observe(raw, bot_id, bot_username, media_enabled)
        } else {
            Ok(Outcome::Refused(super::admission::Refusal::SystemSender))
        };
    }
    observe(
        raw,
        bot_id,
        bot_username,
        media_enabled,
        admission.native_bot_mentions(),
    )
}
/// Normalize one authenticated update. `media_enabled` is the operator's inbound
/// media switch; off, media updates are refused exactly as before.
pub fn observe(
    raw: Value,
    bot_id: &str,
    bot_username: Option<&str>,
    media_enabled: bool,
    allow_bots: bool,
) -> Result<Outcome, VerificationError> {
    let invalid = || VerificationError("telegram_update_invalid");
    let update_id = raw["update_id"]
        .as_i64()
        .filter(|id| *id >= 0)
        .ok_or_else(invalid)?;
    let callback = raw.get("callback_query");
    let message = callback
        .and_then(|cb| cb.get("message"))
        .or_else(|| raw.get("message"))
        .or_else(|| raw.get("edited_message"))
        .or_else(|| raw.get("channel_post"))
        .or_else(|| raw.get("edited_channel_post"));
    let Some(message) = message else {
        return Ok(Outcome::Refused(super::admission::Refusal::SystemEvent));
    };
    let from = callback
        .and_then(|cb| cb.get("from"))
        .unwrap_or(&message["from"]);
    let chat_id = message["chat"]["id"]
        .as_i64()
        .filter(|id| *id != 0)
        .ok_or_else(invalid)?
        .to_string();
    let Some(sender_id) = from["id"].as_i64().filter(|id| *id > 0) else {
        return Ok(Outcome::Refused(super::admission::Refusal::UnknownSender));
    };
    let sender_id = sender_id.to_string();
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
    // The synthetic envelope matches what NyxID would relay for the same
    // message, so one observer checks both transports: a media message has
    // NyxID's media type and its caption as text.
    let media_type = media_content_type(message);
    let shared = match shared_facts::extract(message) {
        Ok(shared) if callback.is_none() => shared,
        Ok(_) => None,
        Err(()) if callback.is_some() => None,
        Err(()) => {
            return Ok(Outcome::Refused(
                super::admission::Refusal::ContentUnsupported,
            ));
        }
    };
    let content_type = if callback.is_some() {
        "interaction"
    } else if shared.is_some() || message.get("text").is_some() {
        "text"
    } else {
        media_type.unwrap_or("unknown")
    };
    let text = match callback {
        Some(_) => None,
        None => shared.map(Value::String).or_else(|| {
            message
                .get("text")
                .or_else(|| {
                    media_type.and(message.get("caption").filter(|caption| caption.is_string()))
                })
                .cloned()
        }),
    };
    let route = crate::telegram::route(bot_id, &chat_id);
    let mut payload: CallbackPayload = serde_json::from_value(json!({
        "message_id":format!("telegram:{bot_id}:{update_id}"),"correlation_id":update_id.to_string(),"platform":"telegram",
        "reply_token":null,"agent":{"api_key_id":bot_id,"name":""},
        "conversation":{"id":route,"platform_id":chat_id,"type":message["chat"]["type"]},
        "sender":{"platform_id":sender_id,"display_name":from.get("first_name")},
        "content":{"type":content_type,"text":text},
        "reply_to_platform_message_id":callback.map(|_|message_id.clone()),"timestamp":timestamp,"raw_platform_data":raw,
    })).map_err(|_| invalid())?;
    // A control character in a sender's name must not refuse the update, or
    // Telegram would redeliver it; validate() keeps its bound as an invariant.
    payload.sender.repair_display_name();
    payload.validate()?;
    match super::relay::telegram::observe_content(
        &payload,
        bot_username,
        Some(bot_id),
        allow_bots,
        MediaIngress::native(media_enabled),
        true,
    )? {
        Observation::Refused(reason) => Ok(Outcome::Refused(reason)),
        Observation::Message(mut facts) => {
            if let Some(ce) = &mut facts.event {
                ce.source = format!("telegram:bot:{bot_id}");
            }
            Ok(Outcome::Admitted(Box::new(Observed {
                payload,
                facts,
                update_id,
                message_id,
            })))
        }
    }
}
/// Build the sealed launch request, or the refusal the media guard chose. An
/// error is answered 401, which makes Telegram redeliver the update, so only a
/// failure unrelated to attachments may produce one.
pub(crate) fn launch(
    channel: Arc<ResolvedChannel>,
    bot_id: &str,
    observed: Observed,
    keys: &MasterKeyRing,
    received: Instant,
    now: i64,
    effects_enabled: bool,
) -> Result<Built, VerificationError> {
    let invalid = || VerificationError("telegram_activity_invalid");
    let Observed {
        payload,
        facts,
        update_id,
        message_id,
    } = observed;
    let telegram_admission = (effects_enabled
        && matches!(
            facts.actor,
            cmaeg_core::activity::ActorKind::Human | cmaeg_core::activity::ActorKind::Bot
        ))
    .then(|| cmaeg_core::seal::TelegramAdmissionProof {
        actor_id: payload.sender.platform_id.clone(),
        actor_kind: facts.actor,
        conversation_kind: facts.conversation,
        mentioned: facts.mentioned,
        replied_to_bot: exact_reply_to_bot(payload.raw_platform_data.as_ref(), bot_id),
    });
    let event_id = EventId::new(format!("telegram:{bot_id}:{update_id}")).map_err(|_| invalid())?;
    let activity = Activity {
        thread: None,
        schema_version: 1,
        event_id: event_id.clone(),
        kind: match facts.event {
            Some(ce) => ActivityKind::Event {
                ce: *ce,
                attachments: Vec::new(),
            },
            None => ActivityKind::Message {
                text: payload.content.text.unwrap_or_default(),
                attachments: facts.attachments,
                mentions_bot: facts.mentioned == Some(true),
                reply_to: None,
                quoted_message: None,
                album_id: facts.album_id,
            },
        },
        actor: Actor {
            id: payload.sender.platform_id,
            display_name: payload.sender.display_name,
            kind: facts.actor,
        },
        conversation: ActivityConversation {
            id: payload.conversation.platform_id.clone(),
            kind: facts.conversation,
            thread_id: facts.thread_id.clone(),
        },
        occurred_at: payload.timestamp,
        source: SourceKind::Telegram {
            platform: cmaeg_core::channel::Platform::Telegram,
            bot_id: bot_id.into(),
        },
        routing: None,
    };
    let activity = match media_guard::admit(activity, "telegram") {
        Ok(Admit::Ok(activity)) => *activity,
        Ok(Admit::Refused(reason)) => return Ok(Built::Refused(reason)),
        Err(_) => return Err(invalid()),
    };
    let partition_key = activity.partition_key(&channel.definition.partition);
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        &partition_key,
    );
    let event_ref = EventRef::seal(
        &EventRefPayload {
            telegram_admission,
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event_id.clone(),
            partition_key,
            conversation_id: conversation_id.clone(),
            expires_at: now + 1800,
            reply: ReplyAddress::Telegram {
                bot_id: bot_id.into(),
                chat_id: payload.conversation.platform_id,
                message_id,
                thread_id: facts.thread_id,
            },
            consumer: None,
            lineage: None,
        },
        keys,
    )
    .map_err(|_| invalid())?;
    Ok(Built::Launch(Box::new(LaunchRequest {
        timing: None,
        consumer: None,
        trace: tracing::Span::current(),
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: received,
    })))
}

/// Effects need evidence about this receiving bot, not merely any bot sender.
fn exact_reply_to_bot(raw: Option<&Value>, bot_id: &str) -> Option<bool> {
    let raw = raw?;
    let parent = if let Some(callback) = raw.get("callback_query") {
        callback.get("message")?
    } else {
        let message = raw.get("message").or_else(|| raw.get("edited_message"))?;
        let Some(parent) = message.get("reply_to_message") else {
            return Some(false);
        };
        parent
    };
    let from = parent.get("from")?;
    Some(from.get("is_bot")?.as_bool()? && from.get("id")?.as_i64()?.to_string() == bot_id)
}

#[cfg(test)]
#[path = "telegram_effect_tests.rs"]
mod effect_tests;
