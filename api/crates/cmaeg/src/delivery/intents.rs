use crate::{
    channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest,
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::{
    ids::{ChannelId, ConsumerId, EventId, MessageRef},
    message::{OutboundMessage, RenderingReport},
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::oneshot;
use utoipa::ToSchema;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplySource {
    Tool,
    Stream,
}
#[derive(Debug)]
pub enum DeliveryCommand {
    Reply {
        trace: tracing::Span,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        message: OutboundMessage,
        source: ReplySource,
        receipt: oneshot::Sender<IntentReceipt>,
    },
    Update {
        trace: tracing::Span,
        channel: Arc<ResolvedChannel>,
        message_ref: MessageRefPayload,
        message: OutboundMessage,
        receipt: oneshot::Sender<IntentReceipt>,
    },
    Typing {
        trace: tracing::Span,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        receipt: oneshot::Sender<TypingReceipt>,
    },
    StreamOutcome {
        turn: Arc<LaunchRequest>,
        outcome: TurnOutcome,
    },
    /// A fan-out child that failed before launch, or was refused at launch; it has no sealed
    /// event_ref the worker could open, so it travels unsealed (G47).
    ConsumerFailed {
        channel: Arc<ResolvedChannel>,
        payload: EventRefPayload,
        consumer: ConsumerId,
        code: &'static str,
        provider_code: Option<String>,
    },
    /// One best-effort notice to the origin chat, using the inbound reply address and token.
    Notice {
        trace: tracing::Span,
        channel: Arc<ResolvedChannel>,
        address: ReplyAddress,
        event_id: EventId,
        text: String,
    },
}
impl DeliveryCommand {
    pub(crate) fn trace(&self) -> tracing::Span {
        match self {
            Self::Reply { trace, .. }
            | Self::Update { trace, .. }
            | Self::Typing { trace, .. }
            | Self::Notice { trace, .. } => trace.clone(),
            Self::StreamOutcome { turn, .. } => turn.trace.clone(),
            Self::ConsumerFailed {
                channel,
                payload,
                consumer,
                ..
            } => tracing::info_span!(
                "cmaeg.deliver",
                channel = %crate::telemetry::channel_hash(channel.id.as_str()),
                event_id = %payload.event_id,
                consumer = %consumer
            ),
        }
    }
    /// The consumer a command acts for, `None` for `default` and for a
    /// notice, which answers no consumer.
    pub fn consumer(&self) -> Option<&ConsumerId> {
        match self {
            Self::Reply { event, .. } | Self::Typing { event, .. } => event.consumer.as_ref(),
            Self::Update { message_ref, .. } => message_ref.consumer.as_ref(),
            Self::StreamOutcome { turn, .. } => turn.consumer.as_ref(),
            Self::ConsumerFailed { consumer, .. } => Some(consumer),
            Self::Notice { .. } => None,
        }
    }
    /// The (channel, event) a command belongs to: the one exhaustive match
    /// that shards commands, so every command of an event reaches the worker
    /// holding its group, and that names the `cmaeg.deliver` span of tools.
    pub fn event_key(&self) -> (&ChannelId, &EventId) {
        match self {
            Self::Reply { event, .. } | Self::Typing { event, .. } => {
                (&event.channel_id, &event.event_id)
            }
            Self::Update { message_ref, .. } => (&message_ref.channel_id, &message_ref.event_id),
            Self::StreamOutcome { turn, .. } => (&turn.channel.id, &turn.activity.event_id),
            Self::ConsumerFailed { payload, .. } => (&payload.channel_id, &payload.event_id),
            Self::Notice {
                channel, event_id, ..
            } => (&channel.id, event_id),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Confirmed { platform_message_id: Option<String> },
    Rejected { reason: String },
    RateLimited { retry_after: u64 },
    Unknown { reason: String },
    Unsupported,
}
impl DeliveryOutcome {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Confirmed { .. } => "confirmed",
            Self::Rejected { .. } => "rejected",
            Self::RateLimited { .. } => "rate_limited",
            Self::Unknown { .. } => "unknown",
            Self::Unsupported => "unsupported",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IntentReceipt {
    pub message_ref: Option<MessageRef>,
    pub rendering: RenderingReport,
    pub delivery: DeliveryOutcome,
}
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TypingReceipt {
    pub outcome: TypingOutcome,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TypingOutcome {
    Sent,
    Unsupported,
}
