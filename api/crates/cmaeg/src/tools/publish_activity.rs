//! Build a hand-off solely from authenticated reference data and bounded input.
use super::publish::{invalid, PublishEventRequest};
use cmaeg_core::{
    activity::{
        Activity, ActivityConversation, ActivityKind, ActivityRouting, Actor, ActorKind, Causation,
        ConsumerTarget, ConversationKind, RouteResolution, SourceKind, ACTIVITY_SCHEMA_VERSION,
    },
    channel::{ChannelDefinition, ConsumerView, Platform, ReplyChoice, Secret, SourceId},
    cloudevent::CloudEvent,
    ids::{ConversationId, EventId},
    problem::Problem,
    seal::{
        event_ref::activity_digest, EventRefPayload, Lineage, MasterKeyRing, ReplyAddress, RootId,
    },
};
use serde_json::json;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

/// Unsealed base for the existing fan-out launcher, which issues each consumer's
/// own reference and conversation identity from this common activity.
pub struct PreparedPublish {
    pub activity: Activity,
    pub payload: EventRefPayload,
}

impl PublishEventRequest {
    /// The gateway-generated identity is stable only when the publisher
    /// supplies the same idempotency key for the same parent and record.
    pub fn event_id(
        &self,
        parent: &EventRefPayload,
        publisher: &cmaeg_core::ids::ConsumerId,
    ) -> EventId {
        let suffix = match &self.idempotency_key {
            Some(key) => {
                let joined = format!(
                    "{}\u{1f}{}\u{1f}{}\u{1f}{}",
                    parent.record_id, parent.event_id, publisher, key
                );
                let digest = ring::digest::digest(&ring::digest::SHA256, joined.as_bytes());
                digest.as_ref()[..16]
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>()
            }
            None => uuid::Uuid::new_v4().simple().to_string(),
        };
        EventId::new(format!("hof_{suffix}")).expect("fixed-size generated event id")
    }
}

pub struct PublishOrigin<'a> {
    pub definition: &'a ChannelDefinition,
    pub parent: &'a EventRefPayload,
    pub publisher: ConsumerView<'a>,
    pub keys: &'a MasterKeyRing,
}

impl PreparedPublish {
    pub fn root(parent: &EventRefPayload) -> Result<RootId, Problem> {
        match &parent.lineage {
            Some(lineage) => RootId::parse(&lineage.root).map_err(|_| invalid("event_ref_invalid")),
            None => Ok(RootId::derive(&parent.record_id, &parent.event_id)),
        }
    }

    pub fn build(
        origin: PublishOrigin<'_>,
        request: &PublishEventRequest,
        targets: Vec<ConsumerTarget>,
        event_id: EventId,
        now: OffsetDateTime,
    ) -> Result<Self, Problem> {
        let PublishOrigin {
            definition,
            parent,
            publisher,
            keys,
        } = origin;
        let routing = definition
            .routing
            .as_ref()
            .ok_or_else(|| invalid("routing_required"))?;
        request.validate(routing.max_targets)?;
        let root = Self::root(parent)?;
        let hop = parent
            .lineage
            .as_ref()
            .map_or(1, |lineage| lineage.hop.saturating_add(1));
        let reply = request.reply.unwrap_or(if routing.default_reply.is_some() {
            ReplyChoice::Default
        } else {
            ReplyChoice::Origin
        });
        let mut path = parent
            .lineage
            .as_ref()
            .map_or_else(Vec::new, |lineage| lineage.path.clone());
        path.push(publisher.id.clone());
        let lineage = Lineage {
            root: root.as_str().into(),
            hop,
            path,
            reply,
        };
        lineage.validate().map_err(|_| invalid("invalid_lineage"))?;
        let source = source(&parent.reply);
        let conversation = match &parent.reply {
            ReplyAddress::Telegram {
                chat_id, thread_id, ..
            } => ActivityConversation {
                id: chat_id.clone(),
                kind: ConversationKind::Custom,
                thread_id: thread_id.clone(),
            },
            _ => ActivityConversation {
                id: format!("handoff_{}", root.as_str()),
                kind: ConversationKind::Custom,
                thread_id: None,
            },
        };
        let occurred_at = now
            .format(&Rfc3339)
            .map_err(|_| invalid("invalid_event_time"))?;
        let causation = Causation {
            parent_event_id: parent.event_id.clone(),
            root: root.as_str().into(),
            published_by: publisher.id.clone(),
            hop,
            reply,
            origin_source_id: source.source_id(),
        };
        let activity = Activity {
            schema_version: ACTIVITY_SCHEMA_VERSION,
            event_id: event_id.clone(),
            kind: ActivityKind::Event {
                ce: CloudEvent {
                    specversion: "1.0".into(),
                    id: event_id.to_string(),
                    source: format!("urn:cmaeg:consumer:{}", publisher.id),
                    r#type: request
                        .event_type
                        .clone()
                        .unwrap_or_else(|| "cmaeg.handoff".into()),
                    time: Some(occurred_at.clone()),
                    subject: None,
                    datacontenttype: Some("application/json".into()),
                    dataschema: None,
                    data: Some(json!({"text":request.text,"data":request.data})),
                    extensions: Default::default(),
                },
                attachments: Vec::new(),
            },
            actor: Actor {
                id: format!("consumer:{}", publisher.id),
                display_name: Some(publisher.label.into()),
                kind: ActorKind::Bot,
            },
            conversation,
            occurred_at,
            source,
            routing: Some(ActivityRouting {
                source_id: SourceId::agent(),
                resolution: RouteResolution::AgentEvent,
                rule_id: None,
                targets,
                addressing_bytes: 0,
                causation: Some(causation),
            }),
        };
        activity
            .validate()
            .map_err(|_| invalid("invalid_published_activity"))?;
        // The default consumer keeps raw-key derivation. A custom subject over
        // 512 bytes is already sealed as a hash, so a hand-off to default uses
        // a different conversation from a direct event with that long subject.
        let conversation_id = ConversationId::derive(
            keys,
            &parent.channel_id,
            definition.partition.version,
            &parent.partition_key,
        );
        let payload = EventRefPayload {
            v: 1,
            activity_sha256: Some(activity_digest(&activity)),
            channel_id: parent.channel_id.clone(),
            record_id: parent.record_id.clone(),
            event_id,
            partition_key: parent.partition_key.clone(),
            conversation_id,
            expires_at: now.unix_timestamp() + 1800,
            reply: inherited_reply(&parent.reply),
            consumer: None,
            lineage: Some(lineage),
        };
        payload
            .validate()
            .map_err(|_| invalid("invalid_published_reference"))?;
        Ok(Self { activity, payload })
    }
}

fn source(reply: &ReplyAddress) -> SourceKind {
    match reply {
        ReplyAddress::Telegram { bot_id, .. } => SourceKind::Telegram {
            platform: Platform::Telegram,
            bot_id: bot_id.clone(),
        },
        ReplyAddress::NyxidRelay {
            platform, route_id, ..
        } => SourceKind::NyxidRelay {
            platform: *platform,
            route_id: route_id.clone(),
        },
        _ => SourceKind::Custom,
    }
}
fn inherited_reply(reply: &ReplyAddress) -> ReplyAddress {
    match reply {
        ReplyAddress::NyxidRelay {
            message_id,
            route_id,
            platform,
            ..
        } => ReplyAddress::NyxidRelay {
            message_id: message_id.clone(),
            route_id: route_id.clone(),
            platform: *platform,
            reply_token: Secret::new(""),
            reply_token_expires_at: 1,
        },
        ReplyAddress::Sync { .. } => ReplyAddress::None,
        _ => reply.clone(),
    }
}
