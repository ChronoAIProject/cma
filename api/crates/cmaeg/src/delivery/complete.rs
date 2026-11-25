//! One bounded NyxID exchange per ordinary logical reply after coordinated rollout.
use super::{
    event::now,
    gate::{expired, within_deadline},
    intents::{DeliveryOutcome, IntentReceipt},
    relay_client::{unknown, Credential, RelayClient},
    render::{LiteralText, RelayText},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::tools::Capabilities;
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};

impl Sender {
    pub(super) async fn complete_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
        interactive: bool,
    ) -> (IntentReceipt, SendEffects) {
        let mut effects = SendEffects {
            first_send_done: ctx.first_send_done,
        };
        let ReplyAddress::NyxidRelay {
            platform,
            route_id,
            reply_token,
            reply_token_expires_at,
            ..
        } = &event.reply
        else {
            return (unsupported(), effects);
        };
        let mut capabilities = Capabilities::relay(*platform);
        if !interactive {
            capabilities.buttons = "numbered_text".into();
        }
        let commonmark = self.relay.commonmark_for(&event.reply);
        let (part, rendering) = if !commonmark && super::render::literal_platform(*platform) {
            LiteralText.whole(message, &capabilities)
        } else {
            RelayText.whole_with_commonmark(message, &capabilities, commonmark)
        };
        if part.text.is_empty()
            && message.native_attachments.is_empty()
            && message.whatsapp_template.is_none()
        {
            return (
                refused(
                    rendering,
                    DeliveryOutcome::Rejected {
                        reason: "empty_message".into(),
                    },
                ),
                effects,
            );
        }
        // Validate the exact bytes that will be dispatched before admission,
        // commitment or claiming the event's single-use token.
        let mut address = event.reply.clone();
        if let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut address {
            *thread_reply = message.thread_reply;
        }
        let prepared = match message.whatsapp_template.as_ref().map_or_else(
            || {
                RelayClient::prepare_native(
                    &address,
                    &part.text,
                    &message.native_attachments,
                    commonmark,
                )
            },
            |template| RelayClient::prepare_whatsapp_template(&address, template),
        ) {
            Ok(prepared) => prepared,
            Err(outcome) => return (refused(rendering, outcome), effects),
        };
        let _ticket = match ctx.gate.enter_route(route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        let Some(mut slot) = within_deadline(ctx.gate, self.pacing.acquire(route_id), None).await
        else {
            return (
                refused(
                    rendering,
                    DeliveryOutcome::Rejected {
                        reason: "pacing_capacity".into(),
                    },
                ),
                effects,
            );
        };
        let permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        if !ctx.gate.commit() {
            return (refused(rendering, expired()), effects);
        }
        // Select after pacing: a token can expire while waiting for this route.
        let use_token = !effects.first_send_done
            && *reply_token_expires_at > now()
            && self.claim_token(reply_token.as_str(), *reply_token_expires_at);
        let credential = if use_token {
            Some(Credential::ReplyToken(reply_token))
        } else {
            ctx.channel
                .definition
                .secrets
                .channel_key
                .as_ref()
                .map(Credential::ChannelKey)
        };
        let Some(credential) = credential else {
            return (
                refused(
                    rendering,
                    DeliveryOutcome::Rejected {
                        reason: "no_credential".into(),
                    },
                ),
                effects,
            );
        };
        effects.first_send_done = true;
        let (mut delivery, outbound) = within_deadline(
            ctx.gate,
            self.relay.send_prepared(credential, &prepared),
            (unknown("side_message_delivery_timeout"), None),
        )
        .await;
        drop(permit);
        if message.whatsapp_template.is_some()
            && matches!(
                delivery,
                DeliveryOutcome::Confirmed {
                    platform_message_id: None
                }
            )
        {
            delivery = unknown("receipt_invalid");
        }
        if let DeliveryOutcome::RateLimited { retry_after } = delivery {
            slot.cooldown(retry_after);
            // NyxID may already have sent earlier platform parts. A bare 429
            // carries no evidence that replaying the logical reply is safe.
            delivery = unknown("rate_limited_unconfirmed");
        }
        ctx.gate.progress();
        tracing::info!(
            commonmark,
            outcome = delivery.kind(),
            "complete relay delivery finished"
        );
        let mut message_ref = if let DeliveryOutcome::Confirmed {
            platform_message_id: Some(id),
        } = &delivery
        {
            MessageRef::seal(
                &MessageRefPayload {
                    discord_message: None,
                    native_media_edit: None,
                    received_reaction: None,
                    native_lark_effect: None,
                    whatsapp_reaction: None,
                    lark_effect: None,
                    telegram_effect: None,
                    relay_message: self
                        .relay
                        .text_edits_enabled()
                        .then(|| {
                            super::edit::destination(
                                *platform,
                                message,
                                &part.text,
                                outbound.clone(),
                            )
                        })
                        .flatten(),
                    channel_id: event.channel_id.clone(),
                    event_id: event.event_id.clone(),
                    platform_message_id: id.clone(),
                    route_id: route_id.clone(),
                    interaction_record_id: None,
                    consumer: event.consumer.clone(),
                },
                &self.keys,
            )
            .ok()
        } else {
            None
        };
        if !message.thread_reply
            && super::edit::destination(*platform, message, &part.text, outbound.clone()).is_some()
        {
            if let (Some(reference), Some(outbound)) = (message_ref.as_ref(), outbound.as_deref()) {
                message_ref = Some(
                    within_deadline(
                        ctx.gate,
                        self.lark_effect_reference(ctx.channel, event, reference.clone(), outbound),
                        reference.clone(),
                    )
                    .await,
                );
            }
        }
        (
            IntentReceipt {
                message_parts: vec![],
                message_ref,
                rendering,
                delivery,
            },
            effects,
        )
    }
}
