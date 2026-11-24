//! Replace only the authenticated original destination.
use super::{
    gate::expired,
    intents::{DeliveryOutcome, IntentReceipt},
    relay_client::unknown,
    render::{LiteralText, RelayText, Renderer},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
    telegram::replaces_telegram_cards,
};
use crate::tools::Capabilities;
use cmaeg_core::{
    channel::{Platform, Source},
    ids::MessageRef,
    message::OutboundMessage,
    seal::{MessageRefPayload, RelayMessageDestination},
};

/// A relay reply can comprise several native messages. Only a bounded, plain
/// text reply is one editable destination; never replace just its first part.
pub(super) fn destination(
    platform: Platform,
    message: &OutboundMessage,
    text: &str,
    outbound: Option<String>,
) -> Option<RelayMessageDestination> {
    if !message.native_attachments.is_empty()
        || message.whatsapp_template.is_some()
        || crate::interactions::has_controls(message)
        || text.is_empty()
        || text.encode_utf16().count() > text_limit(platform)?
    {
        return None;
    }
    Some(RelayMessageDestination {
        platform,
        outbound_message_id: outbound?,
    })
}

fn text_limit(platform: Platform) -> Option<usize> {
    match platform {
        Platform::Discord => Some(2000),
        Platform::Telegram | Platform::Lark | Platform::Feishu => Some(4000),
        _ => None,
    }
}

impl Sender {
    pub async fn update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        super::trace::observe(
            ctx.trace,
            true,
            self.update_inner(ctx, reference, message),
            |(receipt, _)| super::trace::receipt(receipt),
        )
        .await
    }

    async fn update_inner(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        if message.structured_content.is_some() {
            return (unsupported(), SendEffects::default());
        }
        if message
            .native_attachments
            .iter()
            .any(|a| crate::media::presentations::special(a.kind))
        {
            return (unsupported(), SendEffects::default());
        }
        if message.clear_native_attachments
            && !(reference.interaction_record_id.is_some()
                && reference.route_id.starts_with("discord:"))
        {
            return (unsupported(), SendEffects::default());
        }
        if crate::interactions::native_lark::is_reference(reference) {
            return (
                self.native_lark_card_update(ctx, reference, message).await,
                SendEffects::default(),
            );
        }
        // A replacement answers no event, so it never marks a first send.
        let effects = SendEffects::default();
        if message.native_media_update.is_some()
            || reference.native_media_edit.is_some()
            || reference.received_reaction.is_some()
            || reference.native_lark_effect.is_some()
            || reference.whatsapp_reaction.is_some()
        {
            return (unsupported(), effects);
        }
        if reference.interaction_record_id.is_some() && reference.route_id.starts_with("discord:") {
            return (
                self.discord_card_update(ctx, reference, message).await,
                effects,
            );
        }
        if reference.discord_message.is_some() {
            return (self.discord_update(ctx, reference, message).await, effects);
        }
        if !message.native_attachments.is_empty()
            || message.thread_reply
            || message.whatsapp_template.is_some()
        {
            return (unsupported(), effects);
        }
        if reference.consumer.is_some()
            && crate::interactions::has_controls(message)
            && !self.relay.consumer_interactions_enabled()
        {
            return (unsupported(), effects);
        }
        let channel = ctx.channel;
        if reference.relay_message.is_some() {
            return (
                self.plain_relay_update(ctx, reference, message).await,
                effects,
            );
        }
        if reference.route_id.starts_with("telegram:")
            && channel
                .definition
                .sources
                .iter()
                .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }))
        {
            return self.native_update(ctx, reference, message).await;
        }
        let Some(store) = &self.relay.interactions else {
            return (unsupported(), effects);
        };
        let mut native_form = false;
        let lark = if let Some(Source::NyxidRelay { platform, .. }) = channel
            .definition
            .relay_source_for_route(&reference.route_id)
        {
            if crate::interactions::lark::supported(*platform) {
                let Some(id) = reference.interaction_record_id.as_deref() else {
                    return (unsupported(), effects);
                };
                let Ok((card, _)) = store.load(id).await else {
                    return (unsupported(), effects);
                };
                if card
                    .lark
                    .as_ref()
                    .is_none_or(|identity| identity.platform != *platform)
                {
                    return (unsupported(), effects);
                }
                native_form = crate::interactions::lark_form::requested(message)
                    && card
                        .lark
                        .as_ref()
                        .is_some_and(|identity| identity.form.is_some());
                true
            } else {
                false
            }
        } else {
            false
        };
        if reference.interaction_record_id.is_none()
            || (!lark && !replaces_telegram_cards(&channel.definition, &reference.route_id))
        {
            return (unsupported(), effects);
        }
        if native_form
            && reference.consumer.is_some()
            && !self.relay.consumer_interactions_enabled()
        {
            return (unsupported(), effects);
        }
        let capabilities = if lark {
            crate::interactions::lark::capabilities()
        } else {
            Capabilities::telegram()
        };
        // Native Telegram updates returned above; this remaining card path is
        // authenticated as either a Lark card or a Telegram relay card.
        let commonmark = !lark && self.relay.commonmark_for_platform(Platform::Telegram);
        let (parts, mut rendering) = if commonmark {
            let (part, report) = RelayText.whole_with_commonmark(message, &capabilities, true);
            (vec![part], report)
        } else {
            RelayText.render(message, &capabilities)
        };
        if lark {
            let render = if native_form {
                crate::interactions::lark::render_form
            } else {
                crate::interactions::lark::render
            };
            let Ok((_, report)) = render(message, "00000000000000000000000000000000", 1) else {
                return (unsupported(), effects);
            };
            rendering = report;
        }
        if !lark && parts.len() != 1 {
            return (
                IntentReceipt {
                    message_parts: vec![],
                    message_ref: None,
                    rendering,
                    delivery: DeliveryOutcome::Rejected {
                        reason: "interactive_message_must_fit_one_part".into(),
                    },
                },
                effects,
            );
        }
        let Some(key) = channel.definition.secrets.channel_key.as_ref() else {
            return (unsupported(), effects);
        };
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        // Commit point (Amendment R3 §R3.1): fencing the old card is this
        // send's first side effect.
        if !ctx.gate.commit() {
            return (refused(rendering, expired()), effects);
        }
        let lark_lock = match self.lark_edit_lock(channel, reference).await {
            Ok(lock) => lock,
            Err(()) => {
                return (
                    refused(rendering, unknown("lark_mutation_unavailable")),
                    effects,
                )
            }
        };
        let preparation = if native_form {
            store
                .replacement_lark_form(channel, reference, message)
                .await
        } else {
            store.replacement(channel, reference, message).await
        };
        let mut prepared = match preparation {
            Ok(p) => p,
            Err(_) => {
                // No provider mutation was dispatched; an invalid edit must not
                // prevent a later delete/reaction on the still-current card.
                if let Some((store, held)) = lark_lock {
                    let _ = store.release(held).await;
                }
                return (
                    IntentReceipt {
                        message_parts: vec![],
                        message_ref: None,
                        rendering,
                        delivery: DeliveryOutcome::Rejected {
                            reason: "interaction_expired_or_invalid".into(),
                        },
                    },
                    effects,
                );
            }
        };
        let Some(destination) = prepared.card.destination.clone() else {
            return (unsupported(), effects);
        };
        let Some(_slot) = self.pacing.acquire(&reference.route_id).await else {
            return (unsupported(), effects);
        };
        let _permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        if reference.lark_effect.is_some()
            && store
                .refresh_lark_message(channel, reference)
                .await
                .is_err()
        {
            return (
                refused(rendering, unknown("lark_mutation_unavailable")),
                effects,
            );
        }
        let delivery = if lark {
            self.relay
                .edit_lark_card(key, &destination, &prepared.markup)
                .await
        } else {
            self.relay
                .edit_with_commonmark(
                    key,
                    &destination,
                    &parts[0].text,
                    &prepared.markup,
                    commonmark,
                )
                .await
        };
        let mut result = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery,
        };
        if matches!(result.delivery, DeliveryOutcome::Confirmed { .. }) {
            match store.bind(&mut prepared, destination).await {
                Ok(updated) => {
                    result.message_ref = Some(if let Some(authority) = &reference.lark_effect {
                        updated
                            .open(&self.keys)
                            .ok()
                            .and_then(|mut opened| {
                                opened.payload.lark_effect = Some(authority.clone());
                                MessageRef::seal(&opened.payload, &self.keys).ok()
                            })
                            .unwrap_or(updated)
                    } else {
                        updated
                    });
                }
                Err(_) => result.delivery = unknown("interaction_receipt_unconfirmed"),
            }
        }
        if let Some(updated) = result
            .message_ref
            .as_ref()
            .filter(|_| reference.lark_effect.is_some())
        {
            if store
                .retain_lark_reference(&mut prepared, updated)
                .await
                .is_err()
            {
                result.message_ref = None;
                result.delivery = unknown("interaction_receipt_unconfirmed");
            }
        }
        if !matches!(result.delivery, DeliveryOutcome::Unknown { .. }) {
            if let Some((store, held)) = lark_lock {
                let _ = store.release(held).await;
            }
        }
        (result, effects)
    }

    async fn plain_relay_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let Some(destination) = &reference.relay_message else {
            return unsupported();
        };
        if reference.validate().is_err()
            || crate::interactions::has_controls(message)
            || reference.channel_id != ctx.channel.id
            || !ctx.channel.definition.sources.iter().any(|source| {
                matches!(source,
                Source::NyxidRelay { platform, route_ids, .. }
                if *platform == destination.platform && route_ids.contains(&reference.route_id))
            })
        {
            return unsupported();
        }
        let Some(key) = ctx.channel.definition.secrets.channel_key.as_ref() else {
            return unsupported();
        };
        let commonmark = self.relay.commonmark_for_platform(destination.platform);
        let capabilities = Capabilities::relay(destination.platform);
        let (part, rendering) =
            if !commonmark && super::render::literal_platform(destination.platform) {
                LiteralText.whole(message, &capabilities)
            } else {
                RelayText.whole_with_commonmark(message, &capabilities, commonmark)
            };
        if part.text.is_empty()
            || (!commonmark
                && text_limit(destination.platform)
                    .is_none_or(|limit| part.text.encode_utf16().count() > limit))
        {
            return refused(
                rendering,
                DeliveryOutcome::Rejected {
                    reason: "message_update_must_fit_one_part".into(),
                },
            );
        }
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return refused(rendering, refusal.outcome()),
        };
        let Some(mut slot) = self.pacing.acquire(&reference.route_id).await else {
            return refused(
                rendering,
                DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                },
            );
        };
        let _permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return refused(rendering, refusal.outcome()),
        };
        if !ctx.gate.commit() {
            return refused(rendering, expired());
        }
        let lark_lock = match self.lark_edit_lock(ctx.channel, reference).await {
            Ok(lock) => lock,
            Err(()) => return refused(rendering, unknown("lark_mutation_unavailable")),
        };
        let delivery = self
            .relay
            .edit_text_with_commonmark(
                key,
                &reference.platform_message_id,
                &destination.outbound_message_id,
                &part.text,
                commonmark,
            )
            .await;
        if let DeliveryOutcome::RateLimited { retry_after } = delivery {
            slot.cooldown(retry_after);
        }
        ctx.gate.progress();
        if !matches!(delivery, DeliveryOutcome::Unknown { .. }) {
            if let Some((store, held)) = lark_lock {
                let _ = store.release(held).await;
            }
        }
        let message_ref = matches!(delivery, DeliveryOutcome::Confirmed { .. })
            .then(|| MessageRef::seal(reference, &self.keys).ok())
            .flatten();
        IntentReceipt {
            message_parts: vec![],
            message_ref,
            rendering,
            delivery,
        }
    }
}
