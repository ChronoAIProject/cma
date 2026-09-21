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
    Reply {
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        message: OutboundMessage,
        source: ReplySource,
        receipt: oneshot::Sender<IntentReceipt>,
    },
    Update {
        channel: Arc<ResolvedChannel>,
        message_ref: MessageRefPayload,
        message: OutboundMessage,
        receipt: oneshot::Sender<IntentReceipt>,
    },
    Typing {
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        receipt: oneshot::Sender<TypingReceipt>,
    },
    StreamOutcome {
        turn: Arc<LaunchRequest>,
        outcome: TurnOutcome,
    },
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
