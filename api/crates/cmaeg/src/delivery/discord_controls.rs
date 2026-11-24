//! Native component issuance shares delivery custody and durable interaction cards.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    render::{LiteralText, Renderer},
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
        context: Option<crate::providers::EventContextSnapshot>,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        if !message.native_attachments.is_empty() && !self.relay.discord_media_controls_enabled() {
            return (unsupported(), effects);
        }
        if message.native_attachments.len() > 1 && !self.relay.discord_media_card_sets {
            return (unsupported(), effects);
        }
        let form = self.relay.discord_forms_enabled()
            && interactions::discord_form::schema(message).is_ok();
        if interactions::discord::multiselect_requested(message)
            && !self.relay.discord_multiselect_enabled()
        {
            return (unsupported(), effects);
        }
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
        let context = match context {
            Some(context) => context,
            None => match store.event_context(ctx.channel, event).await {
                Ok(context) => context,
                Err(_) => return (unsupported(), effects),
            },
        };
        if !interactions::discord::eligible(ctx.channel, event, &context.activity)
            || (message.thread_reply && context.activity.conversation.thread_id.is_none())
            || (!form
                && interactions::discord::render(message, "00000000000000000000000000000000", 1)
                    .is_err())
        {
            return (unsupported(), effects);
        }
        let capabilities = Capabilities {
            buttons: "inline_keyboard".into(),
            ..Capabilities::native_discord()
        };
        let (parts, mut rendering) = LiteralText.render(message, &capabilities);
        if form {
            interactions::discord_form::report(message, &mut rendering);
        }
        if parts.len() != 1 {
            return (unsupported(), effects);
        }
        let rejected = |reason: &str| IntentReceipt {
            message_parts: vec![],
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
        let target_receipt = match self.discord_reaction_receipt(ctx.channel, event).await {
            Ok(receipt) => receipt,
            Err(_) => return (rejected("reaction_target_custody_unavailable"), effects),
        };
        // Prepare the bounded upload before committing delivery or creating a card.
        // It is sent in the same native request as the opaque controls.
        let files = match crate::discord::outbound_media::prepare(&message.native_attachments).await
        {
            Ok(files) => files,
            Err(_) => return (rejected("attachment_preparation_failed"), effects),
        };
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
        let prepared = if !files.is_empty() {
            store
                .prepare_discord_media(ctx.channel, event, message, context, thread, &files)
                .await
        } else if form {
            store
                .prepare_discord_form(ctx.channel, event, message, context, thread)
                .await
        } else {
            store
                .prepare_with_discord_thread(ctx.channel, event, message, context, thread)
                .await
        };
        let Ok(mut card) = prepared else {
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
            .send_components_result(
                token,
                bot_id,
                channel_id,
                Some(message_id),
                &parts[0].text,
                &files,
                Some(&card.markup),
            )
            .await;
        drop(permit);
        ctx.gate.progress();
        match sent {
            Ok(body) => {
                let id = body["id"]
                    .as_str()
                    .expect("REST confirmed message ID")
                    .to_owned();
                if self.relay.discord_media_card_edits_enabled() {
                    if let Some(identity) = card.card.discord.as_mut().filter(|identity| {
                        identity.media_items().is_ok_and(|items| !items.is_empty())
                    }) {
                        let kinds: Vec<_> = identity
                            .media_items()
                            .unwrap()
                            .iter()
                            .map(|media| media.kind)
                            .collect();
                        let confirmed = crate::discord::rest::media_card_snapshots(
                            &body,
                            bot_id,
                            channel_id,
                            &id,
                            &kinds,
                            &parts[0].text,
                            &card.markup,
                        )
                        .ok()
                        .is_some_and(|snapshots| identity.confirm_media(&snapshots).is_ok());
                        if !confirmed {
                            return (
                                refused(
                                    rendering,
                                    DeliveryOutcome::Unknown {
                                        reason: "discord_media_card_receipt_unconfirmed".into(),
                                    },
                                ),
                                effects,
                            );
                        }
                    }
                }
                let target_confirmed = match &target_receipt {
                    Some(receipt) => receipt.confirm(&id).await.is_ok(),
                    None => true,
                };
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
                        message_parts: vec![],
                        message_ref: reference.as_ref().ok().cloned(),
                        rendering,
                        delivery: if reference.is_ok() && target_confirmed {
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

/// Decide representational eligibility before native form state or platform I/O.
pub(super) fn form_eligible(message: &OutboundMessage) -> bool {
    interactions::discord_form::schema(message).is_ok()
        && LiteralText
            .render(message, &Capabilities::native_discord())
            .0
            .len()
            == 1
}
