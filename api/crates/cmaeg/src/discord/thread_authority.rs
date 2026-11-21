//! Lifecycle scope comes from immutable native inbox evidence and the selected route.
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
    seal::{EventRefPayload, MasterKeyRing, ReplyAddress, ThreadRefPayload},
};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Identity {
    pub id: String,
    pub kind: u8,
    pub guild: String,
    pub parent: Option<String>,
}
impl Identity {
    pub fn thread(&self) -> bool {
        matches!(self.kind, 10..=12) && self.parent.is_some()
    }
    pub fn child(&self, id: &str) -> Option<Self> {
        let kind = match self.kind {
            0 => 11,
            5 => 10,
            _ => return None,
        };
        Some(Self {
            id: id.into(),
            kind,
            guild: self.guild.clone(),
            parent: Some(self.id.clone()),
        })
    }
}
pub(crate) struct Authority {
    pub event: EventRefPayload,
    pub activity: Activity,
    pub origin: Identity,
    pub bot: String,
    pub message: String,
    pub custody: String,
}
impl Authority {
    pub fn target(&self, reference: &ThreadRefPayload) -> Result<Identity, Problem> {
        if !reference.valid() || reference.event != self.event || reference.custody != self.custody
        {
            return Err(denied());
        }
        let target = Identity {
            id: reference.thread_id.clone(),
            kind: reference.thread_type,
            guild: reference.guild_id.clone(),
            parent: Some(reference.parent_id.clone()),
        };
        if self.origin.thread() {
            if target != self.origin {
                return Err(denied());
            }
        } else if self.origin.child(&target.id).as_ref() != Some(&target) {
            // Public only. A bot's visibility never grants the actor private sibling access.
            return Err(denied());
        }
        Ok(target)
    }
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
    if event.validate().is_err()
        || event.lineage.is_some()
        || event.expires_at <= now
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
    if !matches!(source, Source::Discord { bot_id: configured, thread_lifecycle: true, .. } if configured == bot_id)
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
        || !human_message(&body.dispatch, &plan.activity.actor.id)
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
        guild_id: Some(guild),
        parent_id,
    } = evidence
    else {
        return Err(unsupported());
    };
    if !matches!(channel_type, 0 | 5 | 10..=12) {
        return Err(unsupported());
    }
    let origin = Identity {
        id: channel_id.clone(),
        kind: *channel_type,
        guild: guild.clone(),
        parent: if matches!(channel_type, 10..=12) {
            parent_id.clone()
        } else {
            None
        },
    };
    let credential = token(channel, bot_id).ok_or_else(denied)?;
    let custody = crate::discord::mutation::hash(&serde_json::json!({
        "domain":"cmaeg.discord-thread-custody.v1", "channel":channel.id,"record":event.record_id,
        "owner":channel.definition.owner,"source":source,"event":event,
        "actor":plan.activity.actor,"origin":origin,"provider":current.provider,
        "agent_key":current.agent_key_hash,"credential":crate::channels::definition::hash_secret(current.credential.unwrap().as_str()),
        "bot_token":crate::channels::definition::hash_secret(credential.as_str())
    }));
    Ok(Authority {
        event: event.clone(),
        activity: plan.activity,
        origin,
        bot: bot_id.clone(),
        message: message_id.clone(),
        custody,
    })
}
pub(crate) fn human_message(value: &serde_json::Value, actor: &str) -> bool {
    value["author"]["id"].as_str() == Some(actor)
        && ["bot", "system"].iter().all(|flag| {
            value["author"]
                .get(*flag)
                .is_none_or(|value| value.as_bool() == Some(false))
        })
        && value.get("webhook_id").is_none_or(|value| value.is_null())
        && matches!(value["type"].as_u64(), Some(0 | 19))
}
pub(crate) fn denied() -> Problem {
    Problem::new(ProblemCode::Forbidden, 403, "thread_authority_revoked")
}
pub(crate) fn unsupported() -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, "thread_scope_unsupported")
}
pub(crate) fn unavailable() -> Problem {
    Problem::new(
        ProblemCode::ProviderUnavailable,
        503,
        "thread_service_unavailable",
    )
}
