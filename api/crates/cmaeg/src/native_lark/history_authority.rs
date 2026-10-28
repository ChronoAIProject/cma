//! Original admission and current custody, independent of message-effect authority.
use super::message::Message;
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::routing_store::{EventIdentity, PlanStore},
    records::RecordClient,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, ActorKind, SourceKind},
    channel::Source,
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
    seal::{EventRefPayload, MasterKeyRing, NativeLarkHistoryCursorPayload, ReplyAddress},
};
use std::sync::Arc;

pub(crate) struct Authority {
    pub activity: Activity,
    pub cursor: NativeLarkHistoryCursorPayload,
}
/// Captured before durable admission. Current credentials cannot replace this proof later.
pub(crate) fn admission_custody(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> Option<String> {
    let (ReplyAddress::NativeLark(reply), SourceKind::NativeLark(identity)) =
        (&event.reply, &activity.source)
    else {
        return None;
    };
    let Source::NativeLark(source) = channel.definition.source_of(&activity.source)? else {
        return None;
    };
    if !source.conversation_history
        || channel.definition.routing.is_none()
        || channel
            .definition
            .creation
            .as_ref()
            .is_some_and(|c| !c.ready)
        || event.lineage.is_some()
        || !event.verifies_activity(activity)
        || !matches!(activity.kind, ActivityKind::Message { album_id: None, .. })
        || activity.actor.kind != ActorKind::Human
        || activity.actor.id == identity.bot_open_id
        || identity != &reply.identity
        || activity.conversation.id != reply.chat_id
        || activity.conversation.thread_id != reply.thread_id
        || super::message::original_id(identity, &reply.message_id)
            .ok()
            .as_ref()
            != Some(&event.event_id)
    {
        return None;
    }
    super::credentials(&channel.definition, &event.reply)?;
    let consumer = channel
        .consumer(event.consumer.as_ref())
        .filter(|c| !c.pending)?;
    // Typed tuple serialization is independent of serde_json's preserve_order feature.
    Some(crate::tools::message_effects::mutation::hash(&(
        "cmaeg.native-lark-history-admission.v1",
        &channel.id,
        &channel.pointer.record_id,
        &channel.definition.owner,
        source,
        event,
        &activity.actor,
        &activity.conversation,
        &channel.definition.partition,
        consumer.provider,
        channel.consumer_key_hash(event.consumer.as_ref())?,
        crate::channels::definition::hash_secret(consumer.credential?.as_str()),
    )))
}
pub(crate) async fn load(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<Authority, Problem> {
    let ReplyAddress::NativeLark(reply) = &event.reply else {
        return Err(unsupported());
    };
    if event.validate().is_err()
        || event.lineage.is_some()
        || event.channel_id != channel.id
        || event.record_id != channel.pointer.record_id
    {
        return Err(denied());
    }
    let identity = EventIdentity {
        owner: channel.definition.owner.subject.clone(),
        channel_id: channel.id.clone(),
        record_id: event.record_id.clone(),
        event_id: event.event_id.clone(),
    };
    let plan = PlanStore::new(records, keys)
        .load(&identity)
        .await
        .map_err(|_| unavailable())?
        .ok_or_else(unsupported)?;
    let selected = event
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer);
    let target = plan
        .targets
        .iter()
        .find(|t| t.launch.consumer == selected)
        .ok_or_else(denied)?;
    let current = channel
        .consumer(event.consumer.as_ref())
        .ok_or_else(denied)?;
    let source = channel
        .definition
        .source_of(&plan.activity.source)
        .ok_or_else(denied)?;
    let custody = admission_custody(channel, event, &plan.activity).ok_or_else(denied)?;
    if plan.identity != identity
        || plan.source != *source
        || target.launch.event != *event
        || target.launch.provider != *current.provider
        || target.launch.history_custody.as_ref() != Some(&custody)
        || event.expires_at > plan.retained_until
        || !event.verifies_activity(&plan.activity)
    {
        return Err(denied());
    }
    let message: Message = serde_json::from_slice(&plan.source_body).map_err(|_| unsupported())?;
    let Message::Text {
        message_id,
        chat_id,
        sender,
        group,
        thread_id,
        mentioned,
        created_ms,
        ..
    } = message
    else {
        return Err(unsupported());
    };
    if message_id != reply.message_id
        || chat_id != reply.chat_id
        || thread_id != reply.thread_id
        || sender != plan.activity.actor.id
        || mentioned != reply.mentioned
        || created_ms <= 0
        || plan.activity.conversation.kind
            != if group {
                cmaeg_core::activity::ConversationKind::Group
            } else {
                cmaeg_core::activity::ConversationKind::Private
            }
        || (!group && thread_id.is_some())
        || time::OffsetDateTime::parse(
            &plan.activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()
        .map(|t| t.unix_timestamp_nanos() / 1_000_000)
            != Some(i128::from(created_ms))
    {
        return Err(denied());
    }
    Ok(Authority {
        activity: plan.activity,
        cursor: NativeLarkHistoryCursorPayload {
            v: 2,
            event_sha256: crate::tools::message_effects::mutation::hash(event),
            custody,
            origin: reply.clone(),
            anchor_created_ms: created_ms,
            before_message_id: reply.message_id.clone(),
            page_token: String::new(),
            last_scanned_ms: None,
            last_scanned_id: None,
            page_size: 25,
            issued_at: 0,
            expires_at: event.expires_at,
        },
    })
}
pub(crate) fn denied() -> Problem {
    Problem::new(
        ProblemCode::Forbidden,
        403,
        "native_lark_history_authority_revoked",
    )
}
pub(crate) fn unsupported() -> Problem {
    Problem::new(
        ProblemCode::InvalidRequest,
        400,
        "history_scope_unsupported",
    )
}
pub(crate) fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "native_lark_history_unavailable",
    )
}
