use super::{
    gate::{expired, SendGate, Ungated, NO_CHAT_ROUTE},
    intents::{DeliveryOutcome, IntentReceipt},
    render::custom,
    sync::SyncRegistry,
    webhook_client::WebhookClient,
};
use crate::{channels::resolver::ResolvedChannel, turns::outcomes::TurnOutcome};
use cmaeg_core::{
    channel::Sink,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRefPayload, ReplyAddress},
};
use serde_json::{json, Value};
use std::sync::Arc;
#[derive(Clone)]
pub struct CustomDelivery {
    pub public_origin: String,
    pub waiters: Arc<SyncRegistry>,
    pub webhooks: WebhookClient,
}
impl CustomDelivery {
    pub fn new(public_origin: String, waiters: Arc<SyncRegistry>) -> Self {
        Self {
            public_origin,
            waiters,
            webhooks: WebhookClient::default(),
        }
    }
    pub async fn reply(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        match self.begin_reply(gate, event, message) {
            Ok(streamed) => {
                self.complete_reply(gate, channel, event, message, streamed)
                    .await
            }
            Err(receipt) => receipt,
        }
    }
    /// Attempt the Sync side effect once, before deciding whether a lost
    /// waiter should fall back to a default destination or the legacy sinks.
    pub(super) fn begin_reply(
        &self,
        gate: &dyn SendGate,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> Result<bool, IntentReceipt> {
        if matches!(event.reply, ReplyAddress::Sync { .. }) && !gate.commit() {
            return Err(IntentReceipt {
                message_ref: None,
                rendering: custom::report(message),
                delivery: expired(),
            });
        }
        let data = custom::data(event, message.text.as_deref(), Some(message), None);
        Ok(self.emit(event, "cmaeg.message", &data, false))
    }
    /// Complete a reply whose Sync attempt already happened. In particular,
    /// a successful Sync reply still reaches the existing result sinks.
    pub(super) async fn complete_reply(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
        streamed: bool,
    ) -> IntentReceipt {
        let data = custom::data(event, message.text.as_deref(), Some(message), None);
        let delivery = self
            .webhook(
                gate,
                channel,
                event,
                "cmaeg.response.message",
                data,
                streamed,
            )
            .await;
        IntentReceipt {
            message_ref: None,
            rendering: custom::report(message),
            delivery,
        }
    }
    /// Post `message` to the result webhook sinks as the channel's default
    /// reply destination, tagged `destination: "default"`. Nothing is emitted
    /// to a Sync origin: the default destination is not the origin.
    pub async fn default_reply(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let mut data = custom::data(event, message.text.as_deref(), Some(message), None);
        custom::to_default(&mut data);
        let delivery = self
            .webhook(gate, channel, event, "cmaeg.response.message", data, false)
            .await;
        IntentReceipt {
            message_ref: None,
            rendering: custom::report(message),
            delivery,
        }
    }
    pub fn stream(&self, event: &EventRefPayload, outcome: &TurnOutcome) -> bool {
        let frame = match outcome {
            TurnOutcome::TextDelta { text } => Some((
                "cmaeg.text.delta",
                json!({"event_id":event.event_id,"conversation_id":event.conversation_id,"text":text}),
            )),
            TurnOutcome::MessageCommitted { text } => Some((
                "cmaeg.message",
                custom::data(
                    event,
                    Some(text),
                    Some(&OutboundMessage {
                        text: Some(text.clone()),
                        blocks: vec![],
                        reply_to: None,
                    }),
                    None,
                ),
            )),
            _ => terminal(event, outcome).map(|(_, data)| {
                (
                    if matches!(outcome, TurnOutcome::Completed { .. }) {
                        "cmaeg.completed"
                    } else {
                        "cmaeg.failed"
                    },
                    data,
                )
            }),
        };
        frame.is_some_and(|(name, data)| self.emit(event, name, &data, outcome.terminal()))
    }
    pub async fn outcome(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        outcome: &TurnOutcome,
    ) {
        self.post_terminal(channel, event, outcome, false).await;
    }
    /// Post a terminal outcome to the result webhook sinks as the channel's
    /// default reply destination, tagged `destination: "default"`. Returns
    /// the delivery, `None` for an outcome that is not terminal.
    pub async fn default_outcome(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        outcome: &TurnOutcome,
    ) -> Option<DeliveryOutcome> {
        self.post_terminal(channel, event, outcome, true).await
    }
    #[tracing::instrument(name="cmaeg.deliver", skip_all, fields(
        channel = %crate::telemetry::channel_hash(channel.id.as_str()), event_id = %crate::telemetry::channel_hash(event.event_id.as_str()),
        consumer = event.consumer.as_ref().map_or("default", |id| id.as_str())))]
    async fn post_terminal(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        outcome: &TurnOutcome,
        default: bool,
    ) -> Option<DeliveryOutcome> {
        let (kind, mut data) = terminal(event, outcome)?;
        if default {
            custom::to_default(&mut data);
        }
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::DELIVERY_DURATION,
            crate::telemetry::names::PLATFORM,
            "custom",
        );
        let delivered = self
            .webhook(&Ungated, channel, event, kind, data, false)
            .await;
        observation.finish(delivered.kind());
        tracing::info!(
            outcome = delivered.kind(),
            "custom terminal delivery finished"
        );
        Some(delivered)
    }
    /// Whether `address` is a Sync origin whose waiter can still take a
    /// frame; `false` for every other address.
    pub fn sync_registered(&self, address: &ReplyAddress) -> bool {
        matches!(address, ReplyAddress::Sync { waiter } if self.waiters.registered(waiter))
    }
    fn emit(
        &self,
        event: &EventRefPayload,
        name: &'static str,
        data: &Value,
        terminal: bool,
    ) -> bool {
        if let ReplyAddress::Sync { waiter } = &event.reply {
            self.waiters.emit(waiter, name, data, terminal)
        } else {
            false
        }
    }
    async fn webhook(
        &self,
        gate: &dyn SendGate,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        kind: &str,
        data: Value,
        streamed: bool,
    ) -> DeliveryOutcome {
        let envelope = custom::result(&self.public_origin, event, kind, data);
        let mut result = if streamed {
            DeliveryOutcome::Confirmed {
                platform_message_id: None,
            }
        } else {
            DeliveryOutcome::Unsupported
        };
        let mut attempted = streamed;
        let mut ticket = None;
        for sink in &channel.definition.reply.sinks {
            let Sink::ResultWebhook { url, hmac_key_id } = sink else {
                continue;
            };
            if ticket.is_none() {
                match gate.enter_route(NO_CHAT_ROUTE).await {
                    Ok(admitted) => ticket = Some(admitted),
                    Err(refusal) => return refusal.outcome(),
                }
            }
            let outcome = match channel
                .definition
                .secrets
                .hmac_secrets
                .iter()
                .find(|(id, _)| id == hmac_key_id)
            {
                Some((_, key)) => self.webhooks.send_gated(url, key, &envelope, gate).await,
                None => DeliveryOutcome::Rejected {
                    reason: "webhook_key_missing".into(),
                },
            };
            result = if attempted
                && (!matches!(result, DeliveryOutcome::Confirmed { .. })
                    || !matches!(outcome, DeliveryOutcome::Confirmed { .. }))
            {
                DeliveryOutcome::Unknown {
                    reason: "partial_sink_delivery".into(),
                }
            } else {
                outcome
            };
            attempted = true;
        }
        result
    }
}
fn terminal(event: &EventRefPayload, outcome: &TurnOutcome) -> Option<(&'static str, Value)> {
    match outcome {
        TurnOutcome::Completed { final_text } => Some((
            "cmaeg.response.completed",
            custom::data(event, final_text.as_deref(), None, None),
        )),
        TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => Some((
            "cmaeg.response.failed",
            custom::data(
                event,
                None,
                None,
                Some(Problem::new(
                    ProblemCode::ProviderUnavailable,
                    502,
                    "provider turn did not complete",
                )),
            ),
        )),
        _ => None,
    }
}

#[cfg(test)]
#[path = "custom_tests.rs"]
mod tests;
