//! Authenticated edits and reaction facts are notifications of saved messages.
use super::{message_context, Outcome};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        admission::{admit, AdmissionFacts, Refusal},
        launch::LaunchRequest,
        message_event::VerifiedMessageEvent,
        routing_store::AdmittedPlan,
        IngressState,
    },
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind},
    channel::Source,
    cloudevent::CloudEvent,
    ids::{EventId, IdempotencyKey},
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc, time::Instant};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) use super::message_context::Context;
#[derive(Debug)]
pub(crate) enum Error {
    Invalid,
    Context,
    Unavailable,
    Refused(Refusal),
}
pub(crate) fn requested(raw: &Value) -> bool {
    [
        "edited_message",
        "edited_channel_post",
        "message_reaction",
        "message_reaction_count",
    ]
    .iter()
    .any(|key| raw.get(key).is_some())
}
fn positive(value: &Value) -> Result<i64, Error> {
    value.as_i64().filter(|id| *id > 0).ok_or(Error::Invalid)
}
fn timestamp(value: &Value, now: i64) -> Result<(i64, String), Error> {
    let date = positive(value)?;
    if date > now + 300 || date <= now - 7 * 86400 {
        return Err(Error::Invalid);
    }
    let text = OffsetDateTime::from_unix_timestamp(date)
        .map_err(|_| Error::Invalid)?
        .format(&Rfc3339)
        .map_err(|_| Error::Invalid)?;
    Ok((date, text))
}
fn event(raw: &Value) -> Result<(&str, &Value), Error> {
    let object = raw.as_object().ok_or(Error::Invalid)?;
    if object.len() != 2 || raw["update_id"].as_i64().is_none_or(|id| id < 0) {
        return Err(Error::Invalid);
    }
    for name in [
        "edited_message",
        "edited_channel_post",
        "message_reaction",
        "message_reaction_count",
    ] {
        if let Some(value) = object.get(name).filter(|value| value.is_object()) {
            return Ok((name, value));
        }
    }
    Err(Error::Refused(Refusal::SystemSender))
}
fn address(value: &Value) -> Result<(String, String), Error> {
    let chat = value["chat"]["id"]
        .as_i64()
        .filter(|id| *id != 0)
        .ok_or(Error::Invalid)?;
    let message = positive(&value["message_id"])?;
    Ok((chat.to_string(), message.to_string()))
}
pub(crate) async fn prepare(
    state: &IngressState,
    channel: Arc<ResolvedChannel>,
    bot: &str,
    raw: &Value,
    bytes: &[u8],
    received: Instant,
    now: i64,
) -> Result<VerifiedMessageEvent, Error> {
    if !channel.definition.sources.iter().any(|source| {
        matches!(source,
        Source::Telegram { bot_id, message_events: true, .. } if bot_id == bot)
    }) {
        return Err(Error::Context);
    }
    let (_, value) = event(raw)?;
    let (chat, message) = address(value)?;
    let original = message_context::original(state, &channel, bot, &chat, &message, now).await?;
    let activity = normalize(&channel, bot, raw, &original, state.media_enabled, now)?;
    let primary = original.targets.first().ok_or(Error::Invalid)?;
    let event_id = activity.event_id.clone();
    let event_ref = EventRef::seal(
        &EventRefPayload {
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.definition.record_id.clone(),
            event_id: event_id.clone(),
            partition_key: primary.launch.event.partition_key.clone(),
            conversation_id: primary.launch.event.conversation_id.clone(),
            expires_at: now + 1800,
            reply: ReplyAddress::Telegram {
                bot_id: bot.into(),
                chat_id: chat,
                message_id: message,
                thread_id: original.activity.conversation.thread_id.clone(),
            },
            consumer: None,
            lineage: None,
            telegram_admission: None,
        },
        &state.master_keys,
    )
    .map_err(|_| Error::Invalid)?;
    Ok(VerifiedMessageEvent {
        outbound_consumer: None,
        request: LaunchRequest {
            timing: None,
            consumer: None,
            trace: tracing::Span::current(),
            channel,
            activity,
            conversation_id: primary.launch.event.conversation_id.clone(),
            event_ref,
            idempotency_key: IdempotencyKey::from_event(&event_id),
            received_at: received,
        },
        source_bytes: bytes.to_vec(),
        received_at: now,
        original: original.identity,
    })
}
fn normalize(
    channel: &ResolvedChannel,
    bot: &str,
    raw: &Value,
    original: &AdmittedPlan,
    media: bool,
    now: i64,
) -> Result<Activity, Error> {
    let (name, value) = event(raw)?;
    let (chat, message) = address(value)?;
    if chat != original.activity.conversation.id {
        return Err(Error::Invalid);
    }
    let Some(Source::Telegram {
        bot_username,
        admission,
        channel_posts,
        ..
    }) = channel.definition.source_of(&original.activity.source)
    else {
        return Err(Error::Invalid);
    };
    // Original wire facts preserve mention/reply evidence even when an edit
    // removes addressing or a reaction contains no message text/thread at all.
    let wire = serde_json::from_slice(&original.source_body).map_err(|_| Error::Invalid)?;
    let Outcome::Admitted(saved) = super::observe_source(
        wire,
        bot,
        bot_username.as_deref(),
        media,
        admission,
        *channel_posts,
    )
    .map_err(|_| Error::Invalid)?
    else {
        return Err(Error::Context);
    };
    if saved.message_id != message
        || saved.payload.sender.platform_id != original.activity.actor.id
        || saved.facts.thread_id != original.activity.conversation.thread_id
    {
        return Err(Error::Invalid);
    }
    let (actor, occurred_at, change) = if matches!(name, "edited_message" | "edited_channel_post") {
        if (name == "edited_message" && value.get("sender_chat").is_some())
            || value.get("business_connection_id").is_some()
            || value.get("guest_query_id").is_some()
        {
            return Err(Error::Refused(Refusal::SystemSender));
        }
        let (edit_date, edited_at) = timestamp(&value["edit_date"], now)?;
        let (created, created_at) = timestamp(&value["date"], now)?;
        if edit_date < created || created_at != original.activity.occurred_at {
            return Err(Error::Invalid);
        }
        let channel_edit = name == "edited_channel_post";
        if channel_edit
            != (original.activity.conversation.kind
                == cmaeg_core::activity::ConversationKind::Channel)
        {
            return Err(Error::Invalid);
        }
        let mut content = value.clone();
        content
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .remove("edit_date");
        let key = if channel_edit {
            "channel_post"
        } else {
            "message"
        };
        let plain = json!({"update_id":raw["update_id"],key:content});
        let observed = match super::observe_source(
            plain,
            bot,
            bot_username.as_deref(),
            media,
            admission,
            *channel_posts,
        )
        .map_err(|_| Error::Invalid)?
        {
            Outcome::Admitted(observed) => observed,
            Outcome::Refused(reason) => return Err(Error::Refused(reason)),
        };
        if observed.payload.sender.platform_id != original.activity.actor.id
            || observed.facts.actor != original.activity.actor.kind
            || observed.facts.conversation != original.activity.conversation.kind
            || observed.facts.thread_id != original.activity.conversation.thread_id
            || observed.facts.event.is_some()
        {
            return Err(Error::Invalid);
        }
        (
            original.activity.actor.clone(),
            edited_at,
            json!({"patch":{"text":observed.payload.content.text.unwrap_or_default(),"attachments":observed.facts.attachments}}),
        )
    } else {
        let (actor, change) = reaction_facts::normalize(name, value, &chat, admission)?;
        (actor, timestamp(&value["date"], now)?.1, change)
    };
    let chat_type = match original.activity.conversation.kind {
        cmaeg_core::activity::ConversationKind::Private => "private",
        cmaeg_core::activity::ConversationKind::Group => saved
            .payload
            .raw_platform_data
            .as_ref()
            .and_then(|raw| raw["message"]["chat"]["type"].as_str())
            .ok_or(Error::Invalid)?,
        cmaeg_core::activity::ConversationKind::Channel => "channel",
        _ => return Err(Error::Refused(Refusal::SystemSender)),
    };
    if value["chat"]["type"] != chat_type {
        return Err(Error::Invalid);
    }
    // Anonymous reactors require open senders in reaction_facts. Always
    // recheck the original author, using channel admission for channel posts.
    let actors = std::iter::once(&original.activity.actor)
        .chain((actor.kind != ActorKind::Unknown).then_some(&actor));
    for admitted_actor in actors {
        let channel_actor = chat_type == "channel" && admitted_actor == &original.activity.actor;
        let admit_actor = if channel_actor {
            super::channel_posts::admit_channel
        } else {
            admit
        };
        admit_actor(
            admission,
            &AdmissionFacts {
                receiving_bot_id: Some(bot),
                sender_id: &admitted_actor.id,
                chat_id: &chat,
                is_group: chat_type != "private",
                is_bot_sender: admitted_actor.kind == ActorKind::Bot,
                is_system: !channel_actor
                    && admitted_actor.kind != ActorKind::Human
                    && admitted_actor.kind != ActorKind::Bot,
                mentioned: saved.facts.mentioned,
                addressed: saved.facts.addressed(admission),
            },
        )
        .map_err(Error::Refused)?;
    }
    let id = format!(
        "telegram:{bot}:{}",
        raw["update_id"].as_i64().ok_or(Error::Invalid)?
    );
    let mut data = change;
    data["message_id"] = message.clone().into();
    data["original_event_id"] = original.identity.event_id.to_string().into();
    data["original_author"] =
        serde_json::to_value(&original.activity.actor).map_err(|_| Error::Invalid)?;
    let activity = Activity {
        schema_version: 1,
        thread: None,
        event_id: EventId::new(&id).map_err(|_| Error::Invalid)?,
        kind: ActivityKind::Event {
            ce: CloudEvent {
                specversion: "1.0".into(),
                id,
                source: format!("telegram:bot:{bot}"),
                r#type: if matches!(name, "edited_message" | "edited_channel_post") {
                    "cma.message.updated.v1"
                } else {
                    "cma.message.reaction.updated.v1"
                }
                .into(),
                time: Some(occurred_at.clone()),
                subject: Some(message),
                datacontenttype: Some("application/json".into()),
                dataschema: None,
                data: Some(data),
                extensions: Default::default(),
            },
            attachments: vec![],
        },
        actor,
        conversation: original.activity.conversation.clone(),
        occurred_at,
        source: original.activity.source.clone(),
        routing: None,
    };
    activity.validate().map_err(|_| Error::Invalid)?;
    Ok(activity)
}
pub(super) fn reactions(value: &Value) -> Result<Vec<Value>, Error> {
    let values = value
        .as_array()
        .filter(|v| v.len() <= 64)
        .ok_or(Error::Invalid)?;
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    for item in values {
        let object = item.as_object().ok_or(Error::Invalid)?;
        let valid = match item["type"].as_str() {
            Some("emoji") => {
                object.len() == 2
                    && item["emoji"].as_str().is_some_and(|text| {
                        !text.is_empty() && text.len() <= 64 && !text.chars().any(char::is_control)
                    })
            }
            Some("custom_emoji") => {
                object.len() == 2
                    && item["custom_emoji_id"].as_str().is_some_and(|id| {
                        !id.is_empty()
                            && id.len() <= 128
                            && id.bytes().all(|byte| byte.is_ascii_digit())
                    })
            }
            Some("paid") => object.len() == 1,
            _ => false,
        };
        let key = cmaeg_core::profile::canonical_json(item.clone());
        if !valid || !seen.insert(key) {
            return Err(Error::Invalid);
        }
        result.push(item.clone());
    }
    result.sort_by_key(|value| cmaeg_core::profile::canonical_json(value.clone()));
    Ok(result)
}

#[path = "reaction_facts.rs"]
mod reaction_facts;

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
