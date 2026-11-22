//! Record the observed transport outcome without granting delivery authority.
use super::{intents::DeliveryOutcome, send::Sender};
use crate::turns::observer::{default_consumer, DeliveryStep, Destination};
use cmaeg_core::seal::{EventRefPayload, ReplyAddress};

impl Sender {
    /// Report the consumer bound to this event reference, at its origin.
    pub async fn observe(
        &self,
        event: &EventRefPayload,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        answer_response_id: Option<&str>,
    ) {
        self.observe_at(
            event,
            step,
            outcome,
            Destination::Origin,
            None,
            answer_response_id,
        )
        .await;
    }
    pub async fn observe_at(
        &self,
        event: &EventRefPayload,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        destination: Destination,
        default: Option<&super::destination::DefaultDestination>,
        answer_response_id: Option<&str>,
    ) {
        self.observer.delivered(
            &event.record_id,
            &event.event_id,
            event
                .consumer
                .as_ref()
                .unwrap_or_else(|| default_consumer()),
            step,
            outcome,
            destination,
        );
        if default.is_none()
            && matches!(
                event.reply,
                ReplyAddress::Sync { .. } | ReplyAddress::ResultWebhook { .. } | ReplyAddress::None
            )
        {
            // Custom delivery reports its actual local queue and each webhook
            // leg separately; the compatibility aggregate is not another send.
            return;
        }
        crate::telemetry::delivery::outcome(step, outcome, destination, &event.reply, default);
        self.observer
            .checkpoint(
                &event.record_id,
                &event.event_id,
                crate::activity::facts::delivery(
                    event
                        .consumer
                        .as_ref()
                        .unwrap_or_else(|| default_consumer())
                        .as_str(),
                    step,
                    outcome,
                    destination,
                    &event.reply,
                    default,
                )
                .with_answer_response(answer_response_id),
            )
            .await;
    }
}
