//! Opt-in awaited sends. Enqueueing alone is never a delivery receipt.
use super::{
    gate::SendGate,
    intents::{DeliveryCommand, DeliveryOutcome, IntentReceipt, ReplySource},
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
    pub source: ReplySource,
    pub default_destination: Option<(super::destination::DefaultDestination, String)>,
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
    pub(crate) custom: Option<super::custom::CustomDelivery>,
}
impl DeliveryCoordinator {
    pub fn new(sink: Arc<dyn DeliverySink>) -> Self {
        Self { sink, custom: None }
    }

    /// Opt in only when routed custom ingress and its process-local waiters are wired.
    pub fn with_custom_stream(mut self, custom: super::custom::CustomDelivery) -> Self {
        self.custom = Some(custom);
        self
    }

    pub fn supports_custom_stream(&self) -> bool {
        self.custom.is_some()
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
        self.reply_from(channel, event, message, gate, ReplySource::Stream)
            .await
    }
    pub async fn reply_from(
        &self,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        message: OutboundMessage,
        gate: Arc<dyn SendGate>,
        source: ReplySource,
    ) -> IntentReceipt {
        let (receipt, received) = oneshot::channel();
        let command = DeliveryCommand::DurableReply(Box::new(DurableReply {
            trace: tracing::Span::current(),
            channel,
            event,
            message,
            gate,
            source,
            default_destination: None,
            receipt,
        }));
        self.enqueue(command, received).await
    }
    pub async fn reply_default(
        &self,
        channel: Arc<ResolvedChannel>,
        event: EventRefPayload,
        message: OutboundMessage,
        gate: Arc<dyn SendGate>,
        destination: super::destination::DefaultDestination,
        operation: String,
    ) -> IntentReceipt {
        let (receipt, received) = oneshot::channel();
        let command = DeliveryCommand::DurableReply(Box::new(DurableReply {
            trace: tracing::Span::current(),
            channel,
            event,
            message,
            gate,
            source: ReplySource::Stream,
            default_destination: Some((destination, operation)),
            receipt,
        }));
        self.enqueue(command, received).await
    }
    async fn enqueue(
        &self,
        command: DeliveryCommand,
        received: oneshot::Receiver<IntentReceipt>,
    ) -> IntentReceipt {
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
