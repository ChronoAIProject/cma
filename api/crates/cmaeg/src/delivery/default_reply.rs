use super::{
    destination::DefaultDestination,
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{refused, unsupported, SendContext, Sender},
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn default_reply(
        &self,
        context: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
        destination: &DefaultDestination,
        operation: &str,
    ) -> IntentReceipt {
        super::trace::observe(
            context.trace,
            !matches!(destination, DefaultDestination::Webhooks { .. }),
            self.default_reply_inner(context, event, message, destination, operation),
            super::trace::receipt,
        )
        .await
    }

    async fn default_reply_inner(
        &self,
        context: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
        destination: &DefaultDestination,
        operation: &str,
    ) -> IntentReceipt {
        if !destination.authorized(context.channel) {
            let mut result = unsupported();
            result.delivery = DeliveryOutcome::Rejected {
                reason: "default_destination_revoked".into(),
            };
            return result;
        }
        // A default address has no authority to reply to a message/thread at
        // the ingress origin, nor to mint origin-bound interaction callbacks.
        if message.structured_content.is_some()
            || message.reply_with_received_sticker
            || message
                .native_attachments
                .iter()
                .any(|a| crate::media::presentations::special(a.kind))
            || message.thread_reply
            || message.reply_to.is_some()
            || crate::interactions::has_controls(message)
        {
            return unsupported();
        }
        match destination {
            DefaultDestination::Telegram {
                bot_id,
                chat_id,
                thread_id,
                ..
            } => {
                let mut addressed = event.clone();
                addressed.telegram_admission = None;
                addressed.reply = ReplyAddress::Telegram {
                    bot_id: bot_id.clone(),
                    chat_id: chat_id.clone(),
                    // Native send_message addresses only chat/thread; this
                    // placeholder is never used as inbound reply authority.
                    message_id: "0".into(),
                    thread_id: thread_id.clone(),
                };
                self.native_reply(context, &addressed, message).await.0
            }
            DefaultDestination::Webhooks { .. } => {
                let mut addressed = event.clone();
                addressed.reply = ReplyAddress::None;
                self.custom
                    .reply(context.gate, context.channel, &addressed, message)
                    .await
            }
            DefaultDestination::Relay {
                platform, route_id, ..
            } => {
                let Some(secret) = context.channel.definition.secrets.channel_key.as_ref() else {
                    return unsupported();
                };
                let Ok((prepared, rendering)) = self
                    .relay
                    .prepare_initiated(*platform, route_id, operation, message)
                else {
                    return unsupported();
                };
                let _ticket = match context.gate.enter_route(route_id).await {
                    Ok(ticket) => ticket,
                    Err(error) => return refused(rendering, error.outcome()),
                };
                let Some(_slot) = self.pacing.acquire(route_id).await else {
                    return refused(
                        rendering,
                        DeliveryOutcome::Rejected {
                            reason: "pacing_capacity".into(),
                        },
                    );
                };
                let permit = match context.gate.attempt().await {
                    Ok(permit) => permit,
                    Err(error) => return refused(rendering, error.outcome()),
                };
                if !context.gate.commit() {
                    return refused(rendering, expired());
                }
                let delivery = self.relay.send_initiated(secret, prepared).await;
                drop(permit);
                context.gate.progress();
                IntentReceipt {
                    message_parts: vec![],
                    message_ref: None,
                    rendering,
                    delivery,
                }
            }
        }
    }
}
