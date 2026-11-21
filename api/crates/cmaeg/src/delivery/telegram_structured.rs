//! Typed geographic/contact replies participate in the ordinary durable send gate.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    send::{unsupported, SendContext, SendEffects, Sender},
    telegram::{outcome, token},
};
use cmaeg_core::{
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{EventRefPayload, ReplyAddress},
};
use std::time::Duration;
use tokio::time::{timeout_at, Instant};

impl Sender {
    pub(super) async fn telegram_structured_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::Telegram {
            bot_id,
            chat_id,
            thread_id,
            ..
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        let Some(content) = &message.structured_content else {
            return (unsupported(), effects);
        };
        if !self.relay.native_telegram_structured_replies || message.validate().is_err() {
            return (unsupported(), effects);
        }
        let Some(store) = &self.relay.interactions else {
            return (unsupported(), effects);
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let proof = crate::telegram::structured::authorized(
            store.channel_records(),
            self.keys.clone(),
            ctx.channel,
            event,
        );
        if !matches!(timeout_at(deadline, proof).await, Ok(true)) {
            return (unsupported(), effects);
        }
        let Some(credential) =
            token(ctx.channel, bot_id).filter(|t| crate::telegram::media_token_matches(t, bot_id))
        else {
            return (unsupported(), effects);
        };
        let route = crate::telegram::route(bot_id, chat_id);
        let mut result = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering: RenderingReport {
                outcome: RenderingOutcome::Rendered,
                blocks: vec![],
            },
            delivery: expired(),
        };
        let _ticket = match timeout_at(deadline, ctx.gate.enter_route(&route)).await {
            Ok(Ok(ticket)) => ticket,
            Ok(Err(error)) => {
                result.delivery = error.outcome();
                return (result, effects);
            }
            Err(_) => return (result, effects),
        };
        let Ok(Some(_slot)) = timeout_at(deadline, self.pacing.acquire(&route)).await else {
            return (result, effects);
        };
        let permit = match timeout_at(deadline, ctx.gate.attempt()).await {
            Ok(Ok(permit)) => permit,
            Ok(Err(error)) => {
                result.delivery = error.outcome();
                return (result, effects);
            }
            Err(_) => return (result, effects),
        };
        if !ctx.gate.commit() {
            return (result, effects);
        }
        result.delivery = match timeout_at(
            deadline,
            self.relay.telegram.send_structured(
                credential,
                bot_id,
                chat_id,
                thread_id.as_deref(),
                content,
            ),
        )
        .await
        {
            Ok(Ok(id)) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            },
            Ok(Err(error)) => outcome(error),
            Err(_) => DeliveryOutcome::Unknown {
                reason: "telegram_response_unconfirmed".into(),
            },
        };
        drop(permit);
        ctx.gate.progress();
        (result, effects)
    }
}
