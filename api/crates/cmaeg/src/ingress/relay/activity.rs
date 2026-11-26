//! Public activity and opaque reply reference created after authentication and admission.
use super::{facts::RelayFacts, verify::VerifiedCallback};
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::{
        launch::LaunchRequest,
        media_guard::{self, Admit, Built},
    },
};
use cmaeg_core::{
    activity::{Activity, ActivityConversation, ActivityKind, Actor, SourceKind},
    channel::Platform,
    ids::{ConversationId, EventId, IdempotencyKey, MessageRef},
    seal::{EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};
/// Build the sealed launch request, or the refusal the media guard chose. The
/// error is the static 400 detail for an activity invalid for reasons unrelated
/// to its attachments.
pub(crate) fn launch_request(
    channel: Arc<ResolvedChannel>,
    platform: Platform,
    verified: VerifiedCallback,
    mut facts: RelayFacts,
    keys: &MasterKeyRing,
    received_at: Instant,
    now: i64,
) -> Result<Built, &'static str> {
    super::lark_resources::bind(&channel, platform, &verified.payload, &mut facts);
    let payload = verified.payload;
    let member_join = facts.event.as_ref().filter(|ce| {
        platform == Platform::Telegram
            && ce.r#type == crate::ingress::telegram::member_joins::EVENT_TYPE
    });
    let event_id = EventId::new(if let Some(callback) = &facts.callback {
        format!(
            "{}-callback:{:x}",
            platform.as_str(),
            Sha256::digest(format!("{}:{}", payload.conversation.id, callback.id).as_bytes())
        )
    } else if let Some(join) = member_join {
        // Keep the full route and update in the default consumer's bounded key.
        let mut digest = Sha256::new();
        digest.update(payload.conversation.id.as_bytes());
        digest.update([0]);
        digest.update(join.id.as_bytes());
        format!("telegram:relay:{:x}", digest.finalize())
    } else {
        payload.message_id.clone()
    })
    .map_err(|_| "activity_invalid")?;
    let member_join = member_join.is_some();
    let occurred_at = facts
        .event
        .as_ref()
        .and_then(|ce| ce.time.clone())
        .unwrap_or(payload.timestamp);
    let kind = match facts.event {
        Some(ce) => ActivityKind::Event {
            ce: *ce,
            attachments: Vec::new(),
        },
        // NyxID derives `content.text` only from a content JSON's top-level
        // `text` key (nyxid `lark.rs:392-395`), so a Lark post's text exists
        // only in the raw event, from which the observer derived it.
        None => ActivityKind::Message {
            text: facts.text.or(payload.content.text).unwrap_or_default(),
            attachments: facts.attachments,
            mentions_bot: facts.mentioned == Some(true),
            reply_to: payload
                .reply_to_message_id
                .map(MessageRef::new)
                .transpose()
                .map_err(|_| "activity_invalid")?,
            quoted_message: None,
            album_id: facts.album_id,
        },
    };
    let thread_reply = facts.thread.is_some();
    let activity = Activity {
        thread: facts.thread.map(|thread| *thread),
        schema_version: 1,
        event_id: event_id.clone(),
        kind,
        actor: Actor {
            id: if member_join {
                "telegram:system".into()
            } else {
                payload.sender.platform_id
            },
            display_name: if member_join {
                None
            } else {
                payload.sender.display_name
            },
            kind: facts.actor,
        },
        conversation: ActivityConversation {
            id: payload.conversation.platform_id,
            kind: facts.conversation,
            thread_id: facts.thread_id,
        },
        occurred_at,
        source: SourceKind::NyxidRelay {
            platform,
            route_id: payload.conversation.id.clone(),
        },
        routing: None,
    };
    let activity = match media_guard::admit(activity, "nyxid_relay") {
        Ok(Admit::Ok(activity)) => *activity,
        Ok(Admit::Refused(reason)) => return Ok(Built::Refused(reason)),
        Err(_) => return Err("activity_invalid"),
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
            telegram_admission: None,
            v: 1,
            activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
            channel_id: channel.id.clone(),
            record_id: channel.pointer.record_id.clone(),
            event_id: event_id.clone(),
            partition_key,
            conversation_id: conversation_id.clone(),
            expires_at: verified.reply_expires_at.min(now.saturating_add(1800)),
            reply: ReplyAddress::NyxidRelay {
                thread_reply,
                message_id: payload.message_id,
                route_id: payload.conversation.id,
                platform,
                reply_token: verified.reply_token,
                reply_token_expires_at: verified.reply_expires_at,
            },
            consumer: None,
            lineage: None,
        },
        keys,
    )
    .map_err(|_| "event_ref_too_large")?;
    Ok(Built::Launch(Box::new(LaunchRequest {
        timing: None,
        consumer: None,
        trace: tracing::Span::current(),
        channel,
        activity,
        conversation_id,
        event_ref,
        idempotency_key: IdempotencyKey::from_event(&event_id),
        received_at,
    })))
}
