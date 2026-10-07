//! Native Telegram update normalization reuses the strict platform observer.
use super::{
    launch::LaunchRequest,
    relay::{
        facts::{Observation, RelayFacts},
        payload::CallbackPayload,
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
pub fn observe(
    raw: Value,
    bot_id: &str,
    bot_username: Option<&str>,
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
    let content_type = if callback.is_some() {
        "interaction"
    } else if message.get("text").is_some() {
        "text"
    } else {
        "unknown"
    };
    let route = crate::telegram::route(bot_id, &chat_id);
    let mut payload: CallbackPayload = serde_json::from_value(json!({
        "message_id":format!("telegram:{bot_id}:{update_id}"),"correlation_id":update_id.to_string(),"platform":"telegram",
        "reply_token":null,"agent":{"api_key_id":bot_id,"name":""},
        "conversation":{"id":route,"platform_id":chat_id,"type":message["chat"]["type"]},
        "sender":{"platform_id":sender_id,"display_name":from.get("first_name")},
        "content":{"type":content_type,"text":if callback.is_none() {message.get("text")} else {None}},
        "reply_to_platform_message_id":callback.map(|_|message_id.clone()),"timestamp":timestamp,"raw_platform_data":raw,
    })).map_err(|_| invalid())?;
    // A control character in a sender's name must not refuse the update, or
    // Telegram would redeliver it; validate() keeps its bound as an invariant.
    payload.sender.repair_display_name();
    payload.validate()?;
    match super::relay::telegram::observe(&payload, bot_username)? {
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
pub fn launch(
    channel: Arc<ResolvedChannel>,
    bot_id: &str,
    observed: Observed,
    keys: &MasterKeyRing,
    received: Instant,
    now: i64,
) -> Result<LaunchRequest, VerificationError> {
    let invalid = || VerificationError("telegram_activity_invalid");
    let Observed {
        payload,
        facts,
        update_id,
        message_id,
    } = observed;
    let event_id = EventId::new(format!("telegram:{bot_id}:{update_id}")).map_err(|_| invalid())?;
    let activity = Activity {
        schema_version: 1,
        event_id: event_id.clone(),
        kind: match facts.event {
            Some(ce) => ActivityKind::Event {
                ce: *ce,
                attachments: Vec::new(),
            },
            None => ActivityKind::Message {
                text: payload.content.text.unwrap_or_default(),
                attachments: vec![],
                mentions_bot: facts.mentioned == Some(true),
                reply_to: None,
                album_id: None,
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
    };
    activity.validate().map_err(|_| invalid())?;
    let partition_key = activity.partition_key(&channel.definition.partition);
    let conversation_id = ConversationId::derive(
        keys,
        &channel.id,
        channel.definition.partition.version,
        &partition_key,
    );
    let event_ref = EventRef::seal(
        &EventRefPayload {
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
        },
        keys,
    )
    .map_err(|_| invalid())?;
    Ok(LaunchRequest {
        timing: None,
        trace: tracing::Span::current(),
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at: received,
    })
}
