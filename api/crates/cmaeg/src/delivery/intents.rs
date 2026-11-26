use crate::{
    channels::resolver::ResolvedChannel, ingress::launch::LaunchRequest,
    turns::outcomes::TurnOutcome,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload},
};
use std::{sync::Arc, time::Instant};
use tokio::sync::oneshot;
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
        accepted_deadline: Option<Instant>,
        /// Identity from this provider stream, independent of Activity's current response.
        answer_response_id: Option<String>,
        recovery: Option<Arc<crate::turns::RecoveryAuthority>>,
    },
    RetireStream {
        turn: Arc<LaunchRequest>,
    },
}
impl DeliveryCommand {
    pub(crate) fn trace(&self) -> tracing::Span {
        match self {
            Self::DurableReply(reply) => reply.trace.clone(),
            Self::Reply { trace, .. } | Self::Update { trace, .. } | Self::Typing { trace, .. } => {
                trace.clone()
            }
            Self::StreamOutcome { turn, .. } | Self::RetireStream { turn } => turn.trace.clone(),
        }
    }
}
pub use cmaeg_engine::{
    ConfirmedMessagePart, DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt,
};
