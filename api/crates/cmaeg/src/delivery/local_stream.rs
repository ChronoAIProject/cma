//! Local queue evidence is deliberately separate from platform delivery receipts.
use super::{
    custom::CustomDelivery, destination::WebhookDestination, gate::SendGate,
    intents::DeliveryOutcome, render::custom,
};
use crate::{channels::resolver::ResolvedChannel, turns::outcomes::TurnOutcome};
use cmaeg_core::{
    channel::Sink,
    message::OutboundMessage,
    problem::{Problem, ProblemCode},
    seal::{EventRefPayload, ReplyAddress},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStreamOutcome {
    /// Durable reservation; whether enqueue happened is not yet recorded.
    Pending,
    /// Only the bounded in-process queue accepted the frame, not the reader.
    Buffered,
    /// No live waiter accepted this attempt, including exhausted local capacity.
    Unavailable,
    /// The process lost the enqueue result. Never retransmitted as a reconnect.
    Lost,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CustomStreamEffect {
    Delta { text: String },
    Message { text: String },
    Completed { final_text: Option<String> },
    Failed,
}
impl CustomStreamEffect {
    pub(crate) fn project(outcome: &TurnOutcome, answer: bool) -> Option<Self> {
        match outcome {
            TurnOutcome::TextDelta { text } if answer => Some(Self::Delta { text: text.clone() }),
            TurnOutcome::MessageCommitted { text } if answer => {
                Some(Self::Message { text: text.clone() })
            }
            TurnOutcome::Completed { final_text } => Some(Self::Completed {
                final_text: answer.then(|| final_text.clone()).flatten(),
            }),
            TurnOutcome::Failed { .. } | TurnOutcome::Incomplete { .. } => Some(Self::Failed),
            _ => None,
        }
    }
    pub(crate) fn terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed)
    }
    pub(crate) fn frame(&self, event: &EventRefPayload) -> (&'static str, Value) {
        match self {
            Self::Delta { text } => (
                "cmaeg.text.delta",
                json!({"event_id":event.event_id,"conversation_id":event.conversation_id,"text":text}),
            ),
            Self::Message { text } => (
                "cmaeg.message",
                custom::data(
                    event,
                    Some(text),
                    Some(&OutboundMessage {
                        text: Some(text.clone()),
                        blocks: vec![],
                        reply_to: None,
                        native_attachments: vec![],
                        whatsapp_template: None,
                        thread_reply: false,
                    }),
                    None,
                ),
            ),
            Self::Completed { final_text } => (
                "cmaeg.completed",
                custom::data(event, final_text.as_deref(), None, None),
            ),
            Self::Failed => (
                "cmaeg.failed",
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
            ),
        }
    }
}
impl CustomDelivery {
    pub(crate) fn lost_local_stream(&self, event: &EventRefPayload) {
        if let ReplyAddress::Sync { waiter } = &event.reply {
            // The original frame is never replayed. A waiter surviving a worker
            // failure receives an explicit failure control; restart has no waiter.
            self.waiters.emit(
                waiter,
                "cmaeg.failed",
                &json!({
                    "event_id": event.event_id, "conversation_id": event.conversation_id,
                    "error": {"code":"internal", "detail":"sync_delivery_lost"},
                }),
                true,
            );
        }
    }
    pub(crate) fn local(
        &self,
        event: &EventRefPayload,
        effect: &CustomStreamEffect,
    ) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        let (name, data) = effect.frame(event);
        if self.waiters.emit(waiter, name, &data, effect.terminal()) {
            LocalStreamOutcome::Buffered
        } else {
            LocalStreamOutcome::Unavailable
        }
    }
    /// One explicit frozen webhook leg. This method never emits a local frame.
    pub(crate) async fn terminal_webhook(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        effect: &CustomStreamEffect,
        destination: &WebhookDestination,
        gate: &dyn SendGate,
    ) -> DeliveryOutcome {
        if !effect.terminal() || !webhook_authorized(channel, destination) {
            return DeliveryOutcome::Rejected {
                reason: "webhook_destination_revoked".into(),
            };
        }
        let Some((_, key)) = channel
            .definition
            .secrets
            .hmac_secrets
            .iter()
            .find(|(id, _)| id == &destination.hmac_key_id)
        else {
            return DeliveryOutcome::Rejected {
                reason: "webhook_key_missing".into(),
            };
        };
        let _ticket = match gate.enter_route(super::gate::NO_CHAT_ROUTE).await {
            Ok(ticket) => ticket,
            Err(refusal) => return refusal.outcome(),
        };
        let kind = if matches!(effect, CustomStreamEffect::Completed { .. }) {
            "cmaeg.response.completed"
        } else {
            "cmaeg.response.failed"
        };
        let (_, data) = effect.frame(event);
        let envelope = custom::result(&self.public_origin, event, kind, data);
        self.webhooks
            .send_gated(&destination.url, key, &envelope, gate)
            .await
    }
}
pub(crate) fn webhook_authorized(
    channel: &ResolvedChannel,
    destination: &WebhookDestination,
) -> bool {
    channel.definition.reply.sinks.iter().any(|sink| matches!(sink,
        Sink::ResultWebhook { url, hmac_key_id } if url == &destination.url && hmac_key_id == &destination.hmac_key_id))
}
