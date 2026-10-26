//! Durable interaction sends; unconfirmed sends never create callback authority.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    relay_client::{unknown, Credential},
    render::{RelayText, Renderer},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::tools::Capabilities;
use cmaeg_core::{
    channel::Platform,
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn interactive_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let mut effects = SendEffects {
            first_send_done: ctx.first_send_done,
        };
        if event.consumer.is_some() && !self.relay.consumer_interactions_enabled() {
            return (unsupported(), effects);
        }
        let lark = matches!(event.reply, ReplyAddress::NyxidRelay { platform, .. }
            if crate::interactions::lark::supported(platform));
        let whatsapp = matches!(
            event.reply,
            ReplyAddress::NyxidRelay {
                platform: Platform::Whatsapp,
                ..
            }
        );
        let capabilities = if lark {
            crate::interactions::lark::capabilities()
        } else {
            Capabilities::telegram()
        };
        let (parts, mut rendering) = RelayText.render(message, &capabilities);
        if lark {
            let Ok((_, report)) =
                crate::interactions::lark::render(message, "00000000000000000000000000000000", 1)
            else {
                return (unsupported(), effects);
            };
            rendering = report;
        }
        if whatsapp {
            let Ok((_, report)) = crate::interactions::whatsapp::render(
                message,
                "00000000000000000000000000000000",
                1,
            ) else {
                return (unsupported(), effects);
            };
            rendering = report;
        }
        let rejected = |reason: &str| IntentReceipt {
            message_ref: None,
            rendering: rendering.clone(),
            delivery: DeliveryOutcome::Rejected {
                reason: reason.into(),
            },
        };
        if !lark && !whatsapp && parts.len() != 1 {
            return (rejected("interactive_message_must_fit_one_part"), effects);
        }
        let Some(key) = ctx.channel.definition.secrets.channel_key.as_ref() else {
            return (rejected("no_channel_credential"), effects);
        };
        let Some(store) = &self.relay.interactions else {
            return (rejected("interactions_unavailable"), effects);
        };
        let cmaeg_core::seal::ReplyAddress::NyxidRelay { route_id, .. } = &event.reply else {
            return (unsupported(), effects);
        };
        let _ticket = match ctx.gate.enter_route(route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        // Commit point (Amendment R3 §R3.1): the durable card write is this
        // send's first side effect.
        if !ctx.gate.commit() {
            return (refused(rendering, expired()), effects);
        }
        let mut prepared = match store.prepare(ctx.channel, event, message).await {
            Ok(p) => p,
            Err(_) => return (rejected("interaction_state_unavailable"), effects),
        };
        let Some(mut slot) = self.pacing.acquire(route_id).await else {
            return (rejected("pacing_capacity"), effects);
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        effects.first_send_done = true;
        let (mut delivery, mut outbound) = if lark {
            self.relay
                .send_lark_card(&event.reply, key, &prepared.markup)
                .await
        } else if whatsapp {
            self.relay
                .send_whatsapp_interactive(&event.reply, key, &prepared.markup)
                .await
        } else {
            self.relay
                .send_rich(
                    &event.reply,
                    Credential::ChannelKey(key),
                    &parts[0].text,
                    Some(&prepared.markup),
                )
                .await
        };
        drop(permit);
        // Only a definite 429 is retryable. Timeout, lost receipts and 5xx may
        // already have sent the message and must not cause an automatic resend.
        if whatsapp {
            if let DeliveryOutcome::RateLimited { retry_after } = delivery {
                slot.cooldown(retry_after);
                slot.retry().await;
                let _retry = match ctx.gate.attempt().await {
                    Ok(permit) => permit,
                    Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
                };
                (delivery, outbound) = self
                    .relay
                    .send_whatsapp_interactive(&event.reply, key, &prepared.markup)
                    .await;
                if let DeliveryOutcome::RateLimited { retry_after } = delivery {
                    slot.cooldown(retry_after);
                }
            }
        }
        ctx.gate.progress();
        let mut result = IntentReceipt {
            message_ref: None,
            rendering,
            delivery,
        };
        if let (
            DeliveryOutcome::Confirmed {
                platform_message_id: Some(platform_id),
            },
            Some(outbound_id),
        ) = (&result.delivery, outbound)
        {
            match store
                .bind(
                    &mut prepared,
                    crate::interactions::Destination {
                        outbound_id,
                        platform_id: platform_id.clone(),
                    },
                )
                .await
            {
                Ok(reference) => result.message_ref = Some(reference),
                Err(_) => result.delivery = unknown("interaction_receipt_unconfirmed"),
            }
        }
        (result, effects)
    }
}
