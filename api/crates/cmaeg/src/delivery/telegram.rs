//! Native chat delivery using only the channel's encrypted platform credential.
use super::{
    gate::{expired, within_deadline},
    intents::{ConfirmedMessagePart, DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt},
    render::{RelayText, Renderer},
    send::{refused, unsupported, SendContext, SendEffects, Sender},
};
use crate::{channels::resolver::ResolvedChannel, telegram::Error, tools::Capabilities};
use cmaeg_core::{
    channel::{ChannelDefinition, Platform, Secret, Source},
    ids::MessageRef,
    message::OutboundMessage,
    seal::{EventRefPayload, MessageRefPayload, ReplyAddress},
};
use serde_json::json;

impl Sender {
    pub(super) async fn native_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        // A native send never marks the event's first send; writing it would
        // change when the relay ack is due.
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
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_telegram());
        let mut receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "empty_message".into(),
            },
        };
        let Some(token) = token(ctx.channel, bot_id) else {
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_destination_revoked".into(),
            };
            return (receipt, effects);
        };
        let route = crate::telegram::route(bot_id, chat_id);
        let _ticket = match ctx.gate.enter_route(&route).await {
            Ok(ticket) => ticket,
            Err(refusal) => {
                receipt.delivery = refusal.outcome();
                return (receipt, effects);
            }
        };
        let effect_issued_at = time::OffsetDateTime::now_utc().unix_timestamp();
        let split_refs = self.relay.native_split_text_refs
            && parts.len() > 1
            && super::discord_edit::editable_content(message);
        let controls = crate::interactions::has_controls(message);
        if controls && event.consumer.is_some() && !self.relay.consumer_interactions_enabled() {
            return (unsupported(), effects);
        }
        let mut prepared = if controls {
            if parts.len() != 1 {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "interactive_message_must_fit_one_part".into(),
                };
                return (receipt, effects);
            }
            let Some(store) = &self.relay.interactions else {
                return (unsupported(), effects);
            };
            // Commit point (Amendment R3 §R3.1): the durable card write is
            // this send's first side effect.
            if !ctx.gate.commit() {
                receipt.delivery = expired();
                return (receipt, effects);
            }
            match store.prepare(ctx.channel, event, message).await {
                Ok(prepared) => Some(prepared),
                Err(_) => {
                    receipt.delivery = DeliveryOutcome::Rejected {
                        reason: "interaction_state_unavailable".into(),
                    };
                    return (receipt, effects);
                }
            }
        } else {
            None
        };
        let mut confirmed = 0;
        let mut first = None;
        for (index, part) in parts.iter().enumerate() {
            let Some(mut slot) = within_deadline(ctx.gate, self.pacing.acquire(&route), None).await
            else {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let permit = match ctx.gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => {
                    receipt.delivery = refusal.outcome();
                    break;
                }
            };
            // Commit point of a text reply: its first request. A reply with
            // controls committed before its card write; committing is idempotent.
            if index == 0 && !ctx.gate.commit() {
                receipt.delivery = expired();
                break;
            }
            let markup = prepared.as_ref().map(|p| &p.markup);
            let send_part = || async {
                let transport = async {
                    if split_refs {
                        self.relay
                            .telegram
                            .send_confirmed_text_part(
                                token,
                                chat_id,
                                thread_id.as_deref(),
                                &part.text,
                            )
                            .await
                    } else {
                        self.relay
                            .telegram
                            .send_message(token, chat_id, thread_id.as_deref(), &part.text, markup)
                            .await
                    }
                };
                within_deadline(ctx.gate, transport, Err(Error::Unconfirmed)).await
            };
            let mut sent = super::trace::native::part(
                index,
                false,
                send_part(),
                super::trace::native::telegram,
            )
            .await;
            drop(permit);
            if let Err(Error::RateLimited(delay)) = sent {
                slot.cooldown(delay);
                within_deadline(ctx.gate, slot.retry(), ()).await;
                let _retry = match ctx.gate.attempt().await {
                    Ok(permit) => permit,
                    Err(refusal) => {
                        receipt.delivery = refusal.outcome();
                        break;
                    }
                };
                sent = super::trace::native::part(
                    index,
                    false,
                    send_part(),
                    super::trace::native::telegram,
                )
                .await;
            }
            ctx.gate.progress();
            match sent {
                Ok(platform_id) => {
                    confirmed += 1;
                    if first.is_none() {
                        first = Some(platform_id.clone());
                    }
                    receipt.delivery = DeliveryOutcome::Confirmed {
                        platform_message_id: first.clone(),
                    };
                    if let Some(prepared) = &mut prepared {
                        let store = self
                            .relay
                            .interactions
                            .as_ref()
                            .expect("prepared interaction store");
                        match store
                            .bind(
                                prepared,
                                crate::interactions::Destination {
                                    outbound_id: platform_id.clone(),
                                    platform_id: platform_id.clone(),
                                },
                            )
                            .await
                        {
                            Ok(reference) => receipt.message_ref = Some(reference),
                            Err(_) => {
                                receipt.delivery = DeliveryOutcome::Unknown {
                                    reason: "interaction_receipt_unconfirmed".into(),
                                };
                                break;
                            }
                        }
                    } else {
                        if receipt.message_ref.is_none() {
                            receipt.message_ref = self.telegram_text_ref(event, &platform_id);
                        }
                        if split_refs {
                            if let Some(message_ref) = self.telegram_part_ref(
                                ctx.channel,
                                event,
                                &platform_id,
                                effect_issued_at,
                            ) {
                                receipt.message_parts.push(ConfirmedMessagePart {
                                    index: index as u16,
                                    message_ref,
                                });
                            }
                        }
                    }
                }
                Err(error) => {
                    receipt.delivery = outcome(error);
                    break;
                }
            }
        }
        if confirmed > 0 && confirmed < parts.len() {
            receipt.delivery = DeliveryOutcome::Unknown {
                reason: "partial_delivery".into(),
            };
        }
        if confirmed == parts.len() && self.relay.telegram_effects_enabled() {
            if let Some(reference) = &receipt.message_ref {
                if let Some(enriched) = crate::tools::message_effects::authority::enrich(
                    reference,
                    event,
                    ctx.channel,
                    &self.keys,
                    effect_issued_at,
                    false,
                ) {
                    receipt.message_ref = Some(enriched);
                }
            }
        }
        (receipt, effects)
    }
    pub(super) async fn native_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
        if let Some(id) = reference.interaction_record_id.as_deref() {
            let Some(store) = &self.relay.interactions else {
                return (unsupported(), effects);
            };
            match store.load(id).await {
                Ok((card, _)) if card.native_telegram_media.is_some() => {
                    return (
                        self.telegram_media_card_update(ctx, reference, message)
                            .await,
                        effects,
                    );
                }
                Err(_) => return (unsupported(), effects),
                _ => {}
            }
        }
        if reference
            .telegram_effect
            .as_ref()
            .is_some_and(|authority| authority.effects_only)
        {
            return (unsupported(), effects);
        }
        let channel = ctx.channel;
        let Some((bot_id, chat_id)) =
            channel
                .definition
                .sources
                .iter()
                .find_map(|source| match source {
                    Source::Telegram { bot_id, .. } => {
                        crate::telegram::route_chat(&reference.route_id, bot_id)
                            .map(|chat| (bot_id.as_str(), chat))
                    }
                    _ => None,
                })
        else {
            return (unsupported(), effects);
        };
        let Some(token) = token(channel, bot_id) else {
            return (unsupported(), effects);
        };
        let (parts, rendering) = RelayText.render(message, &Capabilities::native_telegram());
        if parts.len() != 1 {
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
        let _ticket = match ctx.gate.enter_route(&reference.route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        let mut prepared = if reference.interaction_record_id.is_some() {
            let Some(store) = &self.relay.interactions else {
                return (unsupported(), effects);
            };
            // Commit point (Amendment R3 §R3.1): fencing the old card is this
            // send's first side effect.
            if !ctx.gate.commit() {
                return (refused(rendering, expired()), effects);
            }
            match store.replacement(channel, reference, message).await {
                Ok(prepared) => Some(prepared),
                Err(_) => {
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
                    )
                }
            }
        } else {
            if crate::interactions::has_controls(message) {
                return (unsupported(), effects);
            }
            None
        };
        let empty = json!({"inline_keyboard":[]});
        let markup = prepared.as_ref().map(|p| &p.markup).unwrap_or(&empty);
        let Some(_slot) = self.pacing.acquire(&reference.route_id).await else {
            return (unsupported(), effects);
        };
        let _permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        // A plain edit's request is its first side effect; after a card write
        // this commit is idempotent.
        if !ctx.gate.commit() {
            return (refused(rendering, expired()), effects);
        }
        let delivery = match self
            .relay
            .telegram
            .edit_message(
                token,
                chat_id,
                &reference.platform_message_id,
                &parts[0].text,
                markup,
            )
            .await
        {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(reference.platform_message_id.clone()),
            },
            Err(error) => outcome(error),
        };
        let mut receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering,
            delivery,
        };
        if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. }) {
            if let Some(prepared) = &mut prepared {
                let Some(destination) = prepared.card.destination.clone() else {
                    return (unsupported(), effects);
                };
                match self
                    .relay
                    .interactions
                    .as_ref()
                    .expect("prepared store")
                    .bind(prepared, destination)
                    .await
                {
                    Ok(reference) => receipt.message_ref = Some(reference),
                    Err(_) => {
                        receipt.delivery = DeliveryOutcome::Unknown {
                            reason: "interaction_receipt_unconfirmed".into(),
                        }
                    }
                }
            } else {
                receipt.message_ref = MessageRef::seal(reference, &self.keys).ok();
            }
        }
        if let (Some(authority), Some(updated)) = (&reference.telegram_effect, &receipt.message_ref)
        {
            if let Ok(mut opened) = updated.open(&self.keys) {
                if opened.payload.platform_message_id == reference.platform_message_id
                    && opened.payload.route_id == reference.route_id
                {
                    opened.payload.telegram_effect = Some(authority.clone());
                    receipt.message_ref = MessageRef::seal(&opened.payload, &self.keys).ok();
                }
            }
        }
        (receipt, effects)
    }
    pub(super) async fn typing(
        &self,
        channel: &ResolvedChannel,
        address: &ReplyAddress,
    ) -> TypingReceipt {
        let mut outcome = TypingOutcome::Unsupported;
        if let ReplyAddress::Telegram {
            bot_id,
            chat_id,
            thread_id,
            ..
        } = address
        {
            if let Some(token) = token(channel, bot_id) {
                if self
                    .relay
                    .telegram
                    .typing(token, chat_id, thread_id.as_deref())
                    .await
                    .is_ok()
                {
                    outcome = TypingOutcome::Sent;
                }
            }
        }
        TypingReceipt { outcome }
    }
}
pub(super) fn token<'a>(channel: &'a ResolvedChannel, bot_id: &str) -> Option<&'a Secret> {
    channel
        .definition
        .sources
        .iter()
        .any(|source| matches!(source,Source::Telegram{bot_id:current,..}if current==bot_id))
        .then_some(())?;
    channel.definition.secrets.telegram_bot_token.as_ref()
}
pub(super) fn outcome(error: Error) -> DeliveryOutcome {
    match error {
        Error::Rejected => DeliveryOutcome::Rejected {
            reason: "telegram_rejected".into(),
        },
        Error::RateLimited(retry_after) => DeliveryOutcome::RateLimited { retry_after },
        Error::Unconfirmed => DeliveryOutcome::Unknown {
            reason: "telegram_response_unconfirmed".into(),
        },
    }
}

/// Whether a card sent on `route_id` can be replaced. Only Telegram relays
/// prepare cards, so the route's own relay must be a Telegram one. The message
/// reference carries the relay route its card was sent on (`route_id`, copied
/// from the sealed reply address when the card was bound), so no reply address
/// is needed. With at most one relay the channel-level check is kept verbatim;
/// the two agree on every legacy definition, whose only relay is the route's.
pub(super) fn replaces_telegram_cards(definition: &ChannelDefinition, route_id: &str) -> bool {
    let relays = definition
        .sources
        .iter()
        .filter(|source| matches!(source, Source::NyxidRelay { .. }))
        .count();
    if relays > 1 {
        matches!(
            definition.relay_source_for_route(route_id),
            Some(Source::NyxidRelay {
                platform: Platform::Telegram,
                ..
            })
        )
    } else {
        definition.relay_platform() == Some(Platform::Telegram)
    }
}
