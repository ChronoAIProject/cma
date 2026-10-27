//! Native component issuance shares delivery custody and durable interaction cards.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::{RelayText, Renderer},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::{
    interactions::{self, Destination},
    tools::Capabilities,
};
use cmaeg_core::{
    message::OutboundMessage,
    seal::{EventRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn discord_controls(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        let ReplyAddress::Discord {
            bot_id,
            channel_id,
            message_id,
            ..
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        let Some(store) = &self.relay.interactions else {
            return (unsupported(), effects);
        };
        if event.consumer.is_some() && !self.relay.consumer_interactions_enabled() {
            return (unsupported(), effects);
        }
        let Ok(context) = store.event_context(ctx.channel, event).await else {
            return (unsupported(), effects);
        };
        if !interactions::discord::eligible(ctx.channel, event, &context.activity)
            || (message.thread_reply && context.activity.conversation.thread_id.is_none())
            || interactions::discord::render(message, "00000000000000000000000000000000", 1)
                .is_err()
        {
            return (unsupported(), effects);
        }
        let capabilities = Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        };
        let (parts, rendering) = RelayText.render(message, &capabilities);
        if parts.len() != 1 {
            return (unsupported(), effects);
        }
        let rejected = |reason: &str| IntentReceipt {
            message_ref: None,
            rendering: rendering.clone(),
            delivery: DeliveryOutcome::Rejected {
                reason: reason.into(),
            },
        };
        let Some(token) = crate::discord::rest::token(ctx.channel, bot_id) else {
            return (rejected("native_destination_revoked"), effects);
        };
        if let Err(error) = self.relay.discord.verify_bot(token, bot_id).await {
            return (refused(rendering, super::discord::outcome(error)), effects);
        }
        let route = interactions::discord::route(bot_id, channel_id);
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        if !ctx.gate.commit() {
            return (refused(rendering, expired()), effects);
        }
        let thread = match self
            .relay
            .discord
            .verify_component_channel(token, channel_id, &context.activity.conversation)
            .await
        {
            Ok(thread) => thread,
            Err(error) => return (refused(rendering, super::discord::outcome(error)), effects),
        };
        let Ok(mut card) = store
            .prepare_with_discord_thread(ctx.channel, event, message, context, thread)
            .await
        else {
            return (rejected("interaction_state_unavailable"), effects);
        };
        let Some(mut slot) = self.pacing.acquire(&route).await else {
            return (rejected("pacing_capacity"), effects);
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        let sent = self
            .relay
            .discord
            .send_components(
                token,
                bot_id,
                channel_id,
                Some(message_id),
                &parts[0].text,
                &[],
                Some(&card.markup),
            )
            .await;
        drop(permit);
        ctx.gate.progress();
        match sent {
            Ok(id) => {
                let reference = store
                    .bind(
                        &mut card,
                        Destination {
                            outbound_id: id.clone(),
                            platform_id: id.clone(),
                        },
                    )
                    .await;
                (
                    IntentReceipt {
                        message_ref: reference.as_ref().ok().cloned(),
                        rendering,
                        delivery: if reference.is_ok() {
                            DeliveryOutcome::Confirmed {
                                platform_message_id: Some(id),
                            }
                        } else {
                            DeliveryOutcome::Unknown {
                                reason: "interaction_receipt_unconfirmed".into(),
                            }
                        },
                    },
                    effects,
                )
            }
            Err(error) => {
                if let crate::discord::RestError::RateLimited(seconds) = error {
                    slot.cooldown(seconds);
                }
                (refused(rendering, super::discord::outcome(error)), effects)
            }
        }
    }
}
