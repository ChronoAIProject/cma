//! Native chat delivery using only the channel's encrypted platform credential.
use super::{
    intents::{DeliveryOutcome, IntentReceipt, TypingOutcome, TypingReceipt},
    render::{RelayText, Renderer},
    send::{unsupported, SendContext, SendEffects, Sender},
};
use crate::{channels::resolver::ResolvedChannel, telegram::Error, tools::Capabilities};
use cmaeg_core::{
    channel::{Secret, Source},
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
        let controls = crate::interactions::has_controls(message);
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
        let route = crate::telegram::route(bot_id, chat_id);
        let mut confirmed = 0;
        let mut first = None;
        for part in &parts {
            let Some(mut slot) = self.pacing.acquire(&route).await else {
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let markup = prepared.as_ref().map(|p| &p.markup);
            let mut sent = self
                .relay
                .telegram
                .send_message(token, chat_id, thread_id.as_deref(), &part.text, markup)
                .await;
            if let Err(Error::RateLimited(delay)) = sent {
                slot.cooldown(delay);
                slot.retry().await;
                sent = self
                    .relay
                    .telegram
                    .send_message(token, chat_id, thread_id.as_deref(), &part.text, markup)
                    .await;
            }
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
                    } else if receipt.message_ref.is_none() {
                        receipt.message_ref = MessageRef::seal(
                            &MessageRefPayload {
                                channel_id: event.channel_id.clone(),
                                event_id: event.event_id.clone(),
                                platform_message_id: platform_id,
                                route_id: route.clone(),
                                interaction_record_id: None,
                            },
                            &self.keys,
                        )
                        .ok();
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
        (receipt, effects)
    }
    pub(super) async fn native_update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let effects = SendEffects::default();
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
                    message_ref: None,
                    rendering,
                    delivery: DeliveryOutcome::Rejected {
                        reason: "interactive_message_must_fit_one_part".into(),
                    },
                },
                effects,
            );
        }
        let mut prepared = if reference.interaction_record_id.is_some() {
            let Some(store) = &self.relay.interactions else {
                return (unsupported(), effects);
            };
            match store.replacement(channel, reference, message).await {
                Ok(prepared) => Some(prepared),
                Err(_) => {
                    return (
                        IntentReceipt {
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
fn token<'a>(channel: &'a ResolvedChannel, bot_id: &str) -> Option<&'a Secret> {
    channel
        .definition
        .sources
        .iter()
        .any(|source| matches!(source,Source::Telegram{bot_id:current,..}if current==bot_id))
        .then_some(())?;
    channel.definition.secrets.telegram_bot_token.as_ref()
}
fn outcome(error: Error) -> DeliveryOutcome {
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
