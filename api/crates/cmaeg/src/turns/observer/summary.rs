//! Project only the fields observation consumers may retain.
use cmaeg_core::{
    activity::{Activity, ActivityKind, AttachmentKind, ConsumerTarget, RouteResolution},
    channel::{SourceId, MAX_TARGETS},
    ids::{ConsumerId, RuleId},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Message,
    Interaction,
    Command,
    Event,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySummary {
    pub at: time::OffsetDateTime,
    pub event_key: String,
    pub source: SourceId,
    pub kind: Kind,
    pub actor_masked: String,
    pub chat_masked: String,
    /// Image, audio, video, and file/other; each count saturates.
    pub attachments: [u8; 4],
    pub routing: Option<RoutingSummary>,
    pub causation: Option<CausationSummary>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutingSummary {
    pub resolution: RouteResolution,
    pub rule: Option<RuleId>,
    pub targets: Vec<ConsumerTarget>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CausationSummary {
    pub parent_key: String,
    pub hop: u8,
    pub publisher: ConsumerId,
}
impl ActivitySummary {
    pub fn of(activity: &Activity) -> Self {
        let (kind, items) = match &activity.kind {
            ActivityKind::Message { attachments, .. } => (Kind::Message, attachments.as_slice()),
            ActivityKind::Event { attachments, .. } => (Kind::Event, attachments.as_slice()),
            ActivityKind::Interaction(_) => (Kind::Interaction, &[][..]),
            ActivityKind::Command { .. } => (Kind::Command, &[][..]),
        };
        let mut attachments = [0u8; 4];
        for item in items {
            let i = match item.kind {
                AttachmentKind::Image => 0,
                AttachmentKind::Audio => 1,
                AttachmentKind::Video => 2,
                AttachmentKind::File | AttachmentKind::Other => 3,
            };
            attachments[i] = attachments[i].saturating_add(1);
        }
        Self {
            at: time::OffsetDateTime::now_utc(),
            event_key: crate::telemetry::channel_hash(activity.event_id.as_str()),
            source: activity
                .routing
                .as_ref()
                .map_or_else(|| activity.source.source_id(), |r| r.source_id.clone()),
            kind,
            actor_masked: masked_id(&activity.actor.id),
            chat_masked: masked_id(&activity.conversation.id),
            attachments,
            routing: activity.routing.as_ref().map(|r| RoutingSummary {
                resolution: r.resolution,
                rule: r.rule_id.clone(),
                targets: r
                    .targets
                    .iter()
                    .take(usize::from(MAX_TARGETS))
                    .cloned()
                    .collect(),
            }),
            causation: activity
                .routing
                .as_ref()
                .and_then(|r| r.causation.as_ref())
                .map(|c| CausationSummary {
                    parent_key: crate::telemetry::channel_hash(c.parent_event_id.as_str()),
                    hop: c.hop,
                    publisher: c.published_by.clone(),
                }),
        }
    }
}

/// Keep four trailing characters, but never reveal a short identifier whole.
pub(super) fn masked_id(value: &str) -> String {
    let count = value.chars().count();
    if count <= 4 {
        return "…".into();
    }
    format!("…{}", value.chars().skip(count - 4).collect::<String>())
}
