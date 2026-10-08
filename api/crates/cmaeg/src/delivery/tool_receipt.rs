//! Complete every tool receipt, including refusals before transport dispatch.
use super::intents::{DeliveryOutcome, IntentReceipt};
use crate::telemetry::{channel_hash, names};
use cmaeg_core::ids::{ChannelId, ConsumerId, EventId};
use tokio::sync::oneshot;

pub(super) struct ToolReceipt {
    sender: oneshot::Sender<IntentReceipt>,
    late: Option<Late>,
}

struct Late {
    op: &'static str,
    channel: String,
    event_id: EventId,
    consumer: ConsumerId,
}

impl ToolReceipt {
    pub(super) fn new(
        sender: oneshot::Sender<IntentReceipt>,
        routed: bool,
        op: &'static str,
        channel: &ChannelId,
        event_id: &EventId,
        consumer: Option<&ConsumerId>,
    ) -> Self {
        Self {
            sender,
            late: routed.then(|| Late {
                op,
                channel: channel_hash(channel.as_str()),
                event_id: event_id.clone(),
                consumer: consumer
                    .cloned()
                    .unwrap_or_else(ConsumerId::default_consumer),
            }),
        }
    }

    pub(super) fn send(self, result: IntentReceipt) {
        if let (Err(result), Some(late)) = (self.sender.send(result), self.late) {
            let outcome = result.delivery.kind();
            metrics::counter!(names::TOOL_SENDS_LATE, names::OP => late.op, names::OUTCOME => outcome)
                .increment(1);
            if matches!(result.delivery, DeliveryOutcome::Confirmed { .. }) {
                tracing::info!(channel = %late.channel, event_id = %late.event_id,
                    consumer = %late.consumer, outcome,
                    "routed tool send finished after its caller was answered");
            } else {
                tracing::warn!(channel = %late.channel, event_id = %late.event_id,
                    consumer = %late.consumer, outcome,
                    "routed tool send finished after its caller was answered");
            }
        }
    }
}
