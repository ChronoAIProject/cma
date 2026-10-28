use crate::{
    channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest,
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::{
    ids::MessageRef,
    message::{OutboundMessage, RenderingReport},
    seal::{EventRefPayload, MessageRefPayload},
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
    DurableReply(Box<super::durable::DurableReply>),
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
}
impl DeliveryCommand {
    pub(crate) fn trace(&self) -> tracing::Span {
        match self {
            Self::DurableReply(reply) => reply.trace.clone(),
            Self::Reply { trace, .. } | Self::Update { trace, .. } | Self::Typing { trace, .. } => {
                trace.clone()
            }
            Self::StreamOutcome { turn, .. } => turn.trace.clone(),
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
    /// Confirmed native split-text parts in rendered order; absent for legacy receipts.
    /// An unknown aggregate outcome can retain confirmed earlier parts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub message_parts: Vec<ConfirmedMessagePart>,
    pub message_ref: Option<MessageRef>,
    pub rendering: RenderingReport,
    pub delivery: DeliveryOutcome,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedMessagePart {
    /// Zero-based position in the rendered text, not a delivery retry counter.
    pub index: u16,
    pub message_ref: MessageRef,
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
