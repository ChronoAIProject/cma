//! Local queue evidence is deliberately separate from platform delivery receipts.
use super::{
    custom::CustomDelivery,
    destination::WebhookDestination,
    gate::{SendGate, NO_CHAT_ROUTE},
    intents::DeliveryOutcome,
    render::custom,
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

#[cfg(test)]
#[path = "local_stream_tests.rs"]
mod tests;

pub use cmaeg_engine::LocalStreamOutcome;

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
    pub(crate) fn webhook(&self) -> bool {
        !matches!(self, Self::Delta { .. })
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
                        structured_content: None,
                        reply_with_received_sticker: false,
                        clear_native_attachments: false,
                        native_media_update: None,
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
    /// A terminal refusal can close the original waiter without sending content
    /// under expired or revoked authority. This never invokes external sinks.
    pub(crate) fn refuse_local(
        &self,
        event: &EventRefPayload,
        refusal: Problem,
    ) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        let data = custom::data(event, None, None, Some(refusal));
        if self.waiters.emit(waiter, "cmaeg.failed", &data, true) {
            LocalStreamOutcome::Buffered
        } else {
            LocalStreamOutcome::Unavailable
        }
    }

    /// Only an explicitly negotiated waiter distinguishes response completion
    /// from final media completion. Existing clients keep their terminal frame.
    pub(crate) fn local_response(
        &self,
        event: &EventRefPayload,
        effect: &CustomStreamEffect,
        response_id: &str,
    ) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        if !self.waiters.media_responses(waiter) {
            return self.local(event, effect);
        }
        let (legacy, mut data) = effect.frame(event);
        if effect.terminal() && !self.waiters.retain_terminal(waiter, legacy, &data) {
            return LocalStreamOutcome::Unavailable;
        }
        data["response_id"] = response_id.into();
        data["media_pending"] = true.into();
        data["media_responses_url"] = media_responses_url(event).into();
        let name = match effect {
            CustomStreamEffect::Completed { .. } => "cmaeg.response.completed",
            CustomStreamEffect::Failed => "cmaeg.response.failed",
            _ => legacy,
        };
        if effect.terminal() {
            self.waiters
                .bound(waiter, std::time::Duration::from_secs(120));
        }
        if self.waiters.emit(waiter, name, &data, false) {
            LocalStreamOutcome::Buffered
        } else {
            LocalStreamOutcome::Unavailable
        }
    }
    pub(crate) fn local_media_response(
        &self,
        event: &EventRefPayload,
        data: &Value,
    ) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        if !self.waiters.media_responses(waiter) {
            return LocalStreamOutcome::Unavailable;
        }
        if self
            .waiters
            .emit(waiter, "cmaeg.media.response", data, false)
        {
            LocalStreamOutcome::Buffered
        } else {
            LocalStreamOutcome::Unavailable
        }
    }
    pub(crate) fn finish_media(&self, event: &EventRefPayload, failed: bool) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        if !self.waiters.media_responses(waiter) {
            return LocalStreamOutcome::Unavailable;
        }
        let (original, mut data) = self
            .waiters
            .original_terminal(waiter)
            .unwrap_or_else(|| ("cmaeg.completed", custom::data(event, None, None, None)));
        let failed = failed || original == "cmaeg.failed";
        self.close_media(event, &mut data, failed)
    }
    /// An authority refusal may close a waiter, but must not re-emit content
    /// that was retained under its formerly valid authorization.
    pub(crate) fn refuse_media(
        &self,
        event: &EventRefPayload,
        refusal: Problem,
    ) -> LocalStreamOutcome {
        let mut data = custom::data(event, None, None, Some(refusal));
        self.close_media(event, &mut data, true)
    }
    fn close_media(
        &self,
        event: &EventRefPayload,
        data: &mut Value,
        failed: bool,
    ) -> LocalStreamOutcome {
        let ReplyAddress::Sync { waiter } = &event.reply else {
            return LocalStreamOutcome::Unavailable;
        };
        if !self.waiters.media_responses(waiter) {
            return LocalStreamOutcome::Unavailable;
        }
        data["media_complete"] = (!failed).into();
        data["status"] = if failed { "incomplete" } else { "completed" }.into();
        if self.waiters.emit(
            waiter,
            if failed {
                "cmaeg.failed"
            } else {
                "cmaeg.completed"
            },
            data,
            true,
        ) {
            LocalStreamOutcome::Buffered
        } else {
            LocalStreamOutcome::Unavailable
        }
    }
    pub(crate) async fn send_media_webhook(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        data: Value,
        destination: &WebhookDestination,
        gate: &dyn SendGate,
    ) -> DeliveryOutcome {
        if !webhook_authorized(channel, destination) {
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
        let _ticket = match gate.enter_route(NO_CHAT_ROUTE).await {
            Ok(ticket) => ticket,
            Err(refusal) => return refusal.outcome(),
        };
        let envelope = custom::result(&self.public_origin, event, "cmaeg.media.response", data);
        self.webhooks
            .send_gated(&destination.url, key, &envelope, gate)
            .await
    }
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
    pub(crate) async fn send_webhook(
        &self,
        channel: &ResolvedChannel,
        event: &EventRefPayload,
        effect: &CustomStreamEffect,
        destination: &WebhookDestination,
        gate: &dyn SendGate,
    ) -> DeliveryOutcome {
        if !effect.webhook() || !webhook_authorized(channel, destination) {
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
        let kind = match effect {
            CustomStreamEffect::Message { .. } => "cmaeg.response.message",
            CustomStreamEffect::Completed { .. } => "cmaeg.response.completed",
            CustomStreamEffect::Failed => "cmaeg.response.failed",
            CustomStreamEffect::Delta { .. } => unreachable!("delta refused before send"),
        };
        let (_, data) = effect.frame(event);
        let envelope = custom::result(&self.public_origin, event, kind, data);
        self.webhooks
            .send_gated(&destination.url, key, &envelope, gate)
            .await
    }
}
fn media_responses_url(event: &EventRefPayload) -> String {
    let mut url = url::Url::parse("https://gateway.invalid/").expect("static origin");
    url.path_segments_mut()
        .expect("static URL supports path")
        .extend([
            "v1",
            "channels",
            event.channel_id.as_str(),
            "events",
            event.event_id.as_str(),
            "media-responses",
        ]);
    url.path().into()
}
pub(crate) fn webhook_authorized(
    channel: &ResolvedChannel,
    destination: &WebhookDestination,
) -> bool {
    channel.definition.reply.sinks.iter().any(|sink| matches!(sink,
        Sink::ResultWebhook { url, hmac_key_id } if url == &destination.url && hmac_key_id == &destination.hmac_key_id))
}
