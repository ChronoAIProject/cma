//! Native reaction deltas have delivery identity, never synthetic producer time.
use super::{
    journal_model::Authority, message_event::Snapshot, normalize::snowflake, GatewayError,
};
use crate::ingress::admission::{admit, AdmissionFacts};
use cmaeg_core::{
    activity::{Activity, ActivityKind, Actor, ActorKind, ConversationKind},
    channel::{AdmissionPolicy, GroupAdmission},
    cloudevent::CloudEvent,
    ids::{ConsumerId, EventId},
};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(super) struct Target {
    pub activity: Activity,
    pub original: EventId,
    pub guild: Option<String>,
    pub outbound_consumer: Option<ConsumerId>,
    pub retained_until: i64,
}
pub(super) fn is_event(name: &str) -> bool {
    matches!(
        name,
        "MESSAGE_REACTION_ADD"
            | "MESSAGE_REACTION_REMOVE"
            | "MESSAGE_REACTION_REMOVE_ALL"
            | "MESSAGE_REACTION_REMOVE_EMOJI"
    )
}
pub(super) fn delivery_id(
    authority: &Authority,
    session: &str,
    sequence: u64,
) -> Result<EventId, GatewayError> {
    let digest = crate::connections::store::index_id(
        "discord-reaction-dispatch-v1",
        &[
            &authority.record,
            &authority.bot_id,
            &authority.revision,
            session,
            &sequence.to_string(),
        ],
    );
    EventId::new(format!("discord:{}:reaction:{digest}", authority.bot_id))
        .map_err(|_| GatewayError::Protocol)
}
/// The authenticated member/user object must prove human status. Discord remove
/// events lack that object; the journal obtains it through a bounded native GET.
pub(super) fn actor(raw: &Value, user: &Value, bot: &str) -> Result<Actor, GatewayError> {
    let id = snowflake(&raw["user_id"])?;
    if snowflake(&user["id"])? != id
        || user["username"]
            .as_str()
            .is_none_or(|name| name.is_empty() || name.len() > 128)
        || id == bot
        || super::facts::bool_fact(user, "bot").map_err(|_| GatewayError::Protocol)?
        || super::facts::bool_fact(user, "system").map_err(|_| GatewayError::Protocol)?
    {
        return Err(GatewayError::Authority);
    }
    Ok(Actor {
        id: id.into(),
        kind: ActorKind::Human,
        display_name: user["global_name"]
            .as_str()
            .or_else(|| user["username"].as_str())
            .map(str::to_owned),
    })
}
pub(super) fn normalize(
    authority: &Authority,
    name: &str,
    raw: &Value,
    target: Target,
    actor: Option<Actor>,
    event_id: EventId,
    now: i64,
) -> Result<Snapshot, GatewayError> {
    let message = snowflake(&raw["message_id"])?;
    let channel = snowflake(&raw["channel_id"])?;
    let guild = raw.get("guild_id").map(snowflake).transpose()?;
    if target.activity.conversation.id != channel
        || target.guild.as_deref() != guild
        || (guild.is_some() && authority.intents & (1 << 10) == 0)
        || (guild.is_none() && authority.intents & (1 << 13) == 0)
    {
        return Err(GatewayError::Authority);
    }
    let original_author = target.activity.actor.clone();
    let target_author = if target.outbound_consumer.is_some() {
        Actor {
            id: authority.bot_id.clone(),
            kind: ActorKind::Bot,
            display_name: None,
        }
    } else {
        original_author
    };
    if raw
        .get("message_author_id")
        .is_some_and(|id| id.as_str() != Some(target_author.id.as_str()))
    {
        return Err(GatewayError::Authority);
    }
    let operation = match name {
        "MESSAGE_REACTION_ADD" => "add",
        "MESSAGE_REACTION_REMOVE" => "remove",
        "MESSAGE_REACTION_REMOVE_ALL" => "clear_all",
        "MESSAGE_REACTION_REMOVE_EMOJI" => "clear_emoji",
        _ => return Err(GatewayError::Protocol),
    };
    let clear = matches!(operation, "clear_all" | "clear_emoji");
    if clear != actor.is_none() {
        return Err(GatewayError::Protocol);
    }
    let actor = actor.unwrap_or(Actor {
        id: "unknown".into(),
        kind: ActorKind::Unknown,
        display_name: None,
    });
    if !clear {
        // Addressing was proved by the affected retained message, not by this
        // delta. Recheck sender/chat permissions without claiming a new mention.
        let mut policy = authority.admission.clone();
        if let AdmissionPolicy::Scoped { groups, .. } = &mut policy {
            if *groups != GroupAdmission::Deny {
                *groups = GroupAdmission::All;
            }
        }
        admit(
            &policy,
            &AdmissionFacts {
                receiving_bot_id: Some(&authority.bot_id),
                mentioned: None,
                sender_id: &actor.id,
                chat_id: channel,
                is_group: target.activity.conversation.kind == ConversationKind::Group,
                is_bot_sender: false,
                is_system: false,
                addressed: None,
            },
        )
        .map_err(|_| GatewayError::Authority)?;
    }
    let mut data = json!({
        "message_id":message, "original_event_id":target.original,
        "target_author":target_author, "target_kind":if target.outbound_consumer.is_some() { "outbound" } else { "inbound" },
        "operation":operation, "representation":"delta",
    });
    if operation != "clear_all" {
        let emoji = &raw["emoji"];
        let custom = emoji
            .get("id")
            .filter(|id| !id.is_null())
            .map(snowflake)
            .transpose()?;
        let name = emoji
            .get("name")
            .filter(|name| !name.is_null())
            .map(|name| {
                name.as_str()
                    .filter(|name| !name.is_empty() && name.len() <= 256)
                    .ok_or(GatewayError::Protocol)
            })
            .transpose()?;
        if custom.is_none() && name.is_none() {
            return Err(GatewayError::Protocol);
        }
        data["emoji"] = json!({"id":custom,"name":name});
        if let Some(animated) = emoji.get("animated") {
            data["emoji"]["animated"] =
                Value::Bool(animated.as_bool().ok_or(GatewayError::Protocol)?);
        }
    }
    if !clear {
        let kind = raw["type"]
            .as_u64()
            .filter(|kind| *kind <= 1)
            .ok_or(GatewayError::Protocol)?;
        let burst = raw["burst"].as_bool().ok_or(GatewayError::Protocol)?;
        if burst != (kind == 1) {
            return Err(GatewayError::Protocol);
        }
        data["reaction_type"] = json!(kind);
        data["burst"] = json!(burst);
    }
    let mut activity = target.activity;
    activity.event_id = event_id;
    activity.actor = actor;
    activity.routing = None;
    if let Some(thread) = activity.thread.as_mut() {
        thread.sender_kind = activity.actor.kind;
    }
    activity.occurred_at = OffsetDateTime::from_unix_timestamp(now)
        .map_err(|_| GatewayError::Protocol)?
        .format(&Rfc3339)
        .map_err(|_| GatewayError::Protocol)?;
    activity.kind = ActivityKind::Event {
        ce: CloudEvent {
            specversion: "1.0".into(),
            id: activity.event_id.to_string(),
            source: format!("urn:cmaeg:discord:{}", authority.bot_id),
            r#type: "cma.message.reaction.updated.v1".into(),
            time: None,
            subject: Some(message.into()),
            datacontenttype: Some("application/json".into()),
            dataschema: None,
            data: Some(data),
            extensions: Default::default(),
        },
        attachments: Vec::new(),
    };
    let snapshot = Snapshot {
        activity,
        original_event_id: target.original,
        outbound_consumer: target.outbound_consumer,
    };
    snapshot.validate(&authority.bot_id, message)?;
    Ok(snapshot)
}
