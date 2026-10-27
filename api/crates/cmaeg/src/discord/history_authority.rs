//! History authority comes only from the retained original native human event.
use super::{inbox_reader::Body, rest::token, thread_context::ChannelEvidence};
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
    seal::{EventRefPayload, HistoryOrigin, MasterKeyRing, ReplyAddress},
};
use std::sync::Arc;

pub(crate) struct Authority {
    pub activity: Activity,
    pub origin: HistoryOrigin,
    pub bot: String,
    pub message: String,
    pub custody: String,
}
pub(crate) async fn load(
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    channel: &ResolvedChannel,
    event: &EventRefPayload,
) -> Result<Authority, Problem> {
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let ReplyAddress::Discord {
        bot_id,
        channel_id,
        message_id,
        thread_id,
    } = &event.reply
    else {
        return Err(unsupported());
    };
    if event.expires_at <= now {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "event_ref_expired",
        ));
    }
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
    if !matches!(source, Source::Discord { bot_id: configured, conversation_history: true, .. } if configured == bot_id)
        || plan.identity != identity
        || plan.source != *source
        || target.launch.event != *event
        || target.launch.provider != *current.provider
        || current.pending
        || current.credential.is_none()
        || !event.verifies_activity(&plan.activity)
        || event.expires_at > plan.retained_until
        || !matches!(plan.activity.kind, ActivityKind::Message { .. })
        || plan.activity.actor.kind != ActorKind::Human
        || plan.activity.source
            != (SourceKind::Discord {
                bot_id: bot_id.clone(),
            })
        || plan.activity.conversation.id != *channel_id
        || plan.activity.conversation.thread_id != *thread_id
    {
        return Err(denied());
    }
    let body: Body = serde_json::from_slice(&plan.source_body).map_err(|_| unsupported())?;
    if body.component.is_some()
        || body.message_event.is_some()
        || body.pending_reaction.is_some()
        || body.channel.owner != channel.definition.owner
        || body.channel.record_id != event.record_id
        || body.channel.partition != channel.definition.partition
        || body.channel.source_of(&plan.activity.source) != Some(source)
        || body
            .channel
            .consumer(event.consumer.as_ref())
            .is_none_or(|old| {
                old.pending
                    || old.provider != current.provider
                    || old.agent_key_hash != current.agent_key_hash
            })
        || body.dispatch["id"].as_str() != Some(message_id)
        || !super::thread_authority::human_message(&body.dispatch, &plan.activity.actor.id)
        || plan.activity.actor.id == *bot_id
        || body.dispatch.get("webhook_id").is_some()
    {
        return Err(denied());
    }
    let evidence = body.native_channel.as_ref().ok_or_else(unsupported)?;
    let mut verified = plan.activity.clone();
    if !evidence.apply(&body.dispatch, &mut verified)
        || verified.conversation != plan.activity.conversation
    {
        return Err(denied());
    }
    let ChannelEvidence::Verified {
        channel_id,
        channel_type,
        guild_id,
        parent_id,
    } = evidence
    else {
        return Err(unsupported());
    };
    let origin = HistoryOrigin {
        channel_id: channel_id.clone(),
        channel_type: *channel_type,
        guild_id: guild_id.clone(),
        parent_id: if matches!(channel_type, 10..=12) {
            parent_id.clone()
        } else {
            None
        },
    };
    if !origin.valid() {
        return Err(unsupported());
    }
    let credential = token(channel, bot_id).ok_or_else(denied)?;
    let custody = crate::discord::mutation::hash(&serde_json::json!({
        "domain":"cmaeg.discord-conversation-history-custody.v1", "channel":channel.id,"record":event.record_id,
        "owner":channel.definition.owner,"source":source,"event":event,
        "actor":plan.activity.actor,"origin":origin,"provider":current.provider,
        "conversation":plan.activity.conversation,"partition":event.partition_key,"partition_rule":channel.definition.partition,"consumer":selected,
        "agent_key":current.agent_key_hash,"credential":crate::channels::definition::hash_secret(current.credential.unwrap().as_str()),
        "bot_token":crate::channels::definition::hash_secret(credential.as_str())
    }));
    Ok(Authority {
        activity: plan.activity,
        origin,
        bot: bot_id.clone(),
        message: message_id.clone(),
        custody,
    })
}
pub(crate) fn denied() -> Problem {
    Problem::new(ProblemCode::Forbidden, 403, "history_authority_revoked")
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
        "history_service_unavailable",
    )
}
