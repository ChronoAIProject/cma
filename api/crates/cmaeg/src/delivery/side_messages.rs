//! Bounded side-message transport with independent route pacing.
use std::sync::Arc;

use cmaeg_core::{message::OutboundMessage, seal::EventRefPayload};
use tokio::sync::Semaphore;

use super::{
    durable::receipt_for,
    gate::SendGate,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{SendContext, Sender},
};
use crate::channels::resolver::ResolvedChannel;

#[derive(Clone)]
pub(crate) struct SideMessageDelivery {
    sender: Arc<Sender>,
    capacity: Arc<Semaphore>,
}

impl SideMessageDelivery {
    pub(super) fn new(answer_sender: &Sender) -> Self {
        Self {
            sender: Arc::new(Sender {
                custom: answer_sender.custom.clone(),
                keys: answer_sender.keys.clone(),
                relay: answer_sender.relay.clone(),
                pacing: Default::default(),
                consumed: Default::default(),
                observer: answer_sender.observer.clone(),
            }),
            capacity: Arc::new(Semaphore::new(4)),
        }
    }

    pub(crate) async fn send(
        &self,
        channel: &Arc<ResolvedChannel>,
        event: &EventRefPayload,
        message: &OutboundMessage,
        gate: &dyn SendGate,
    ) -> IntentReceipt {
        let Ok(_permit) = self.capacity.try_acquire() else {
            return receipt_for(DeliveryOutcome::Rejected {
                reason: "side_message_capacity".into(),
            });
        };
        let trace = tracing::Span::current();
        let context = SendContext {
            trace: &trace,
            channel,
            first_send_done: true,
            gate,
        };
        self.sender.reply(&context, event, message).await.0
    }
}
