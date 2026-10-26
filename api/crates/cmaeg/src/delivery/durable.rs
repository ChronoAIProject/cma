//! Opt-in awaited sends. Enqueueing alone is never a delivery receipt.
use super::{
    gate::SendGate,
    intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt},
    router::{DeliverySink, Refused},
    send::unsupported,
};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use std::sync::Arc;
use tokio::sync::oneshot;

pub struct DurableReply {
    pub trace: tracing::Span,
    pub channel: Arc<ResolvedChannel>,
    pub event: EventRefPayload,
    pub message: OutboundMessage,
    pub gate: Arc<dyn SendGate>,
    pub receipt: oneshot::Sender<IntentReceipt>,
}
impl std::fmt::Debug for DurableReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DurableReply").finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct DeliveryCoordinator {
    sink: Arc<dyn DeliverySink>,
}
impl DeliveryCoordinator {
    pub fn new(sink: Arc<dyn DeliverySink>) -> Self {
        Self { sink }
    }

    /// The caller persists intent before calling, and the returned receipt
    /// afterwards. A dropped worker cannot prove whether the platform accepted.
    pub async fn reply(
        &self,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        message: OutboundMessage,
        gate: Arc<dyn SendGate>,
    ) -> IntentReceipt {
        let (receipt, received) = oneshot::channel();
        let command = DeliveryCommand::DurableReply(Box::new(DurableReply {
            trace: tracing::Span::current(),
            channel,
            event,
            message,
            gate,
            receipt,
        }));
        if let Some(result) = self.sink.preflight(&command) {
            return result;
        }
        if let Err(refusal) = self.sink.try_submit(command) {
            return receipt_for(DeliveryOutcome::Rejected {
                reason: match refusal.reason {
                    Refused::Capacity => "delivery_capacity",
                    Refused::Closed => "delivery_closed",
                }
                .into(),
            });
        }
        received.await.unwrap_or_else(|_| {
            receipt_for(DeliveryOutcome::Unknown {
                reason: "delivery_receipt_lost".into(),
            })
        })
    }
}

pub(crate) fn receipt_for(delivery: DeliveryOutcome) -> IntentReceipt {
    let mut result = unsupported();
    result.delivery = delivery;
    result
}
