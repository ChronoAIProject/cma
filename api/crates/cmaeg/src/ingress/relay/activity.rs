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
    seal::{EventRef, EventRefPayload, ReplyAddress},
};
use std::{sync::Arc, time::Instant};
/// Build the sealed launch request, or the refusal the media guard chose. The
/// error is the static 400 detail for an activity invalid for reasons unrelated
/// to its attachments.
pub(crate) fn launch_request(
    channel: Arc<ResolvedChannel>,
    platform: Platform,
    verified: VerifiedCallback,
    facts: RelayFacts,
    state: &crate::ingress::IngressState,
    received_at: Instant,
    now: i64,
) -> Result<Built, &'static str> {
    let keys = &state.master_keys;
    let callback = facts.callback.is_some();
    let payload = verified.payload;
    let event_id = EventId::new(if let Some(callback) = &facts.callback {
        use sha2::{Digest, Sha256};
        format!(
            "telegram-callback:{:x}",
            Sha256::digest(format!("{}:{}", payload.conversation.id, callback.id).as_bytes())
        )
    } else {
        payload.message_id.clone()
    })
    .map_err(|_| "activity_invalid")?;
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
            album_id: facts.album_id,
        },
    };
    let activity = Activity {
        schema_version: 1,
        event_id: event_id.clone(),
        kind,
        actor: Actor {
            id: payload.sender.platform_id,
            display_name: payload.sender.display_name,
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
    let mut activity = match media_guard::admit(activity, "nyxid_relay") {
        Ok(Admit::Ok(activity)) => *activity,
        Ok(Admit::Refused(reason)) => return Ok(Built::Refused(reason)),
        Err(_) => return Err("activity_invalid"),
    };
    let reply = ReplyAddress::NyxidRelay {
        message_id: payload.message_id,
        route_id: payload.conversation.id,
        platform,
        reply_token: verified.reply_token,
        reply_token_expires_at: verified.reply_expires_at,
    };
    // Card ownership is admitted by InteractionStore before a click is consumed.
    if !callback {
        if let Err(reason) = crate::ingress::routing::chat(state, &channel, &mut activity, &reply) {
            return Ok(Built::Refused(reason));
        }
    }
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
            expires_at: verified.reply_expires_at.min(now.saturating_add(1800)),
            reply,
            consumer: None,
            lineage: None,
        },
        keys,
    )
    .map_err(|_| "event_ref_too_large")?;
    Ok(Built::Launch(Box::new(LaunchRequest {
        sync_fanout: false,
        lease: std::sync::Mutex::new(None),
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
