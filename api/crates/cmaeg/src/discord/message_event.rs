//! Native message changes are notifications about an admitted original message.
use super::{
    inbox_reader::Body,
    normalize::{normalize_with_projections, Normalized},
    GatewayError,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, Actor, ActorKind, SourceKind},
    cloudevent::CloudEvent,
    ids::EventId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Snapshot {
    pub activity: Activity,
    pub original_event_id: EventId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outbound_consumer: Option<cmaeg_core::ids::ConsumerId>,
}
impl Snapshot {
    pub fn validate(&self, bot: &str, message: &str) -> Result<(), GatewayError> {
        let ActivityKind::Event { ce, .. } = &self.activity.kind else {
            return Err(GatewayError::Protocol);
        };
        let reaction = ce.r#type == "cma.message.reaction.updated.v1";
        if (!reaction && self.outbound_consumer.is_some())
            || (self.outbound_consumer.is_none()
                && self.original_event_id.as_str() != format!("discord:{bot}:{message}"))
            || self.activity.source != (SourceKind::Discord { bot_id: bot.into() })
            || self.activity.routing.is_some()
            || !matches!(
                ce.r#type.as_str(),
                "cma.message.updated.v1"
                    | "cma.message.deleted.v1"
                    | "cma.message.reaction.updated.v1"
            )
            || ce.id != self.activity.event_id.as_str()
            || ce.subject.as_deref() != Some(message)
            || ce
                .data
                .as_ref()
                .and_then(|data| data["original_event_id"].as_str())
                != Some(self.original_event_id.as_str())
            || ce
                .data
                .as_ref()
                .and_then(|data| data["message_id"].as_str())
                != Some(message)
            || !self.activity.event_id.as_str().starts_with(&if reaction {
                format!("discord:{bot}:reaction:")
            } else {
                format!("discord:{bot}:{message}:")
            })
            || self.activity.validate().is_err()
        {
            return Err(GatewayError::Protocol);
        }
        Ok(())
    }
}

/// Missing author/content fields are legal for a native update. They never
/// establish a new sender or source: all target facts come from the old inbox.
pub(super) fn normalize_change(
    body: &Body,
    bot: &str,
    name: &str,
    raw: &Value,
    now: i64,
) -> Result<Option<Snapshot>, GatewayError> {
    let message = super::protocol::snowflake(&raw["id"]).ok_or(GatewayError::Protocol)?;
    if body.component.is_some()
        || body.message_event.is_some()
        || raw["channel_id"] != body.dispatch["channel_id"]
        || raw.get("guild_id") != body.dispatch.get("guild_id")
    {
        return Ok(None);
    }
    let Normalized::Activity(original) = body.activity(bot)? else {
        return Ok(None);
    };
    let original_id = original.event_id.clone();
    let mut activity = *original;
    let mut data = json!({"message_id":message,"original_event_id":original_id,"original_author":activity.actor});
    let (kind, revision, time, attachments) = if name == "MESSAGE_UPDATE" {
        if raw.get("author").is_some_and(|author| {
            author["id"].as_str() != Some(activity.actor.id.as_str())
                || author.get("bot").is_some_and(|flag| flag != false)
                || author.get("system").is_some_and(|flag| flag != false)
        }) || raw.get("webhook_id").is_some()
            || raw
                .get("type")
                .is_some_and(|kind| !matches!(kind.as_u64(), Some(0 | 19)))
            || (raw.get("content").is_none() && raw.get("attachments").is_none())
        {
            return Ok(None);
        }
        let Some(timestamp) = raw["edited_timestamp"].as_str() else {
            return Ok(None);
        };
        let Ok(edited) = OffsetDateTime::parse(timestamp, &Rfc3339) else {
            return Ok(None);
        };
        // A native edit has a producer revision. Embed-only refreshes without a
        // revision do not become agent work; replay never uses gateway sequence.
        let created = (message.parse::<u64>().map_err(|_| GatewayError::Protocol)? >> 22) as i64
            + 1_420_070_400_000;
        if edited.unix_timestamp_nanos() / 1_000_000 < i128::from(created)
            || edited.unix_timestamp() > now + 300
        {
            return Ok(None);
        }
        let mut merged = body.dispatch.clone();
        let mut patch = serde_json::Map::new();
        if let Some(text) = raw.get("content") {
            if text.as_str().is_none_or(|text| text.len() > 32 * 1024) {
                return Ok(None);
            }
            merged["content"] = text.clone();
            patch.insert("text".into(), text.clone());
        }
        if let Some(files) = raw.get("attachments") {
            merged["attachments"] = files.clone();
        }
        let policy = body
            .channel
            .sources
            .iter()
            .find_map(|source| match source {
                cmaeg_core::channel::Source::Discord {
                    bot_id, admission, ..
                } if bot_id == bot => Some(admission),
                _ => None,
            })
            .ok_or(GatewayError::Authority)?;
        // Admission facts are the original's; edited mentions cannot reroute it.
        let Normalized::Activity(updated) = normalize_with_projections(
            &merged,
            bot,
            policy,
            body.content_projection.as_ref(),
            body.forward_projection.as_ref(),
        )?
        else {
            return Ok(None);
        };
        let ActivityKind::Message { attachments, .. } = updated.kind else {
            return Ok(None);
        };
        let attachments = if raw.get("attachments").is_some() {
            patch.insert(
                "attachments".into(),
                serde_json::to_value(&attachments).map_err(|_| GatewayError::Protocol)?,
            );
            attachments
        } else {
            Vec::new()
        };
        data["patch"] = Value::Object(patch);
        (
            "cma.message.updated.v1",
            format!("edit:{}", edited.unix_timestamp_nanos()),
            edited,
            attachments,
        )
    } else if name == "MESSAGE_DELETE" {
        // Discord does not expose the deleting actor. The original author is
        // retained only as affected-message context, never impersonated.
        activity.actor = Actor {
            id: "unknown".into(),
            kind: ActorKind::Unknown,
            display_name: None,
        };
        if let Some(thread) = activity.thread.as_mut() {
            thread.sender_kind = ActorKind::Unknown;
        }
        (
            "cma.message.deleted.v1",
            "delete".into(),
            OffsetDateTime::from_unix_timestamp(now).map_err(|_| GatewayError::Protocol)?,
            Vec::new(),
        )
    } else {
        return Ok(None);
    };
    activity.event_id = EventId::new(format!("discord:{bot}:{message}:{revision}"))
        .map_err(|_| GatewayError::Protocol)?;
    activity.occurred_at = time.format(&Rfc3339).map_err(|_| GatewayError::Protocol)?;
    activity.kind = ActivityKind::Event {
        ce: CloudEvent {
            specversion: "1.0".into(),
            id: activity.event_id.to_string(),
            source: format!("urn:cmaeg:discord:{bot}"),
            r#type: kind.into(),
            time: (name == "MESSAGE_UPDATE").then(|| activity.occurred_at.clone()),
            subject: Some(message.into()),
            datacontenttype: Some("application/json".into()),
            dataschema: None,
            data: Some(data),
            extensions: Default::default(),
        },
        attachments,
    };
    let snapshot = Snapshot {
        activity,
        original_event_id: original_id,
        outbound_consumer: None,
    };
    snapshot.validate(bot, message)?;
    Ok(Some(snapshot))
}
