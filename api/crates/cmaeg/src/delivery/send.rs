use super::{
    event::now,
    gate::{expired, SendGate},
    intents::{DeliveryOutcome, IntentReceipt},
    pacing::Pacing,
    relay_client::{unknown, Credential, RelayClient},
    render::{RelayText, Renderer},
};
use crate::{
    channels::resolver::ResolvedChannel,
    tools::Capabilities,
    turns::observer::{default_consumer, ActivityObserver, DeliveryStep, Destination},
};
use cmaeg_core::{
    channel::{ChannelDefinition, Platform, Source},
    ids::MessageRef,
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{EventRefPayload, MasterKeyRing, MessageRefPayload, ReplyAddress},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
/// What a send may read about its event. A send never mutates shared delivery state, so a
/// scheduler can run sends for different consumers of one event concurrently.
pub(super) struct SendContext<'a> {
    pub trace: &'a tracing::Span,
    pub channel: &'a Arc<ResolvedChannel>,
    pub first_send_done: bool,
    /// Called at every send path's first side effect; the legacy lane passes `Ungated`.
    pub gate: &'a dyn SendGate,
}
/// State changes a finished send asks its scheduler to apply.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct SendEffects {
    pub first_send_done: bool,
}
pub(super) struct Sender {
    pub custom: super::custom::CustomDelivery,
    pub keys: Arc<MasterKeyRing>,
    pub relay: RelayClient,
    pub pacing: Pacing,
    // Token-use tombstones outlive the short event state but retain no credentials.
    pub consumed: Mutex<HashMap<[u8; 32], i64>>,
    /// Receives each delivery step's outcome; a no-op unless the console is on.
    pub observer: Arc<dyn ActivityObserver>,
}
impl Sender {
    /// Report a legacy delivery: the default consumer, to the event's origin.
    pub fn observe(&self, event: &EventRefPayload, step: DeliveryStep, outcome: &DeliveryOutcome) {
        self.observer.delivered(
            &event.record_id,
            &event.event_id,
            default_consumer(),
            step,
            outcome,
            Destination::Origin,
        );
    }

    #[tracing::instrument(name="cmaeg.deliver", parent = ctx.trace, skip_all, fields(
        channel = %crate::telemetry::channel_hash(ctx.channel.id.as_str()), event_id = %event.event_id))]
    pub async fn reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::DELIVERY_DURATION,
            crate::telemetry::names::PLATFORM,
            match &event.reply {
                ReplyAddress::NyxidRelay { platform, .. } => platform.as_str(),
                ReplyAddress::Telegram { .. } => "telegram",
                _ => "custom",
            },
        );
        let (result, effects) = self.reply_inner(ctx, event, message).await;
        observation.finish(result.delivery.kind());
        (result, effects)
    }
    async fn reply_inner(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        if matches!(event.reply, ReplyAddress::Telegram { .. }) {
            return self.native_reply(ctx, event, message).await;
        }
        let ReplyAddress::NyxidRelay { platform, .. } = &event.reply else {
            let result = self
                .custom
                .reply(ctx.gate, ctx.channel, event, message)
                .await;
            return (result, SendEffects::default());
        };
        // Native controls and their updates are proven only for Telegram; every
        // other platform receives text through NyxID's generic relay reply.
        let interactive = *platform == Platform::Telegram && self.relay.interactions.is_some();
        if interactive && crate::interactions::has_controls(message) {
            return self.interactive_reply(ctx, event, message).await;
        }
        // The send's own view of the event's first-send flag, returned as its effect.
        let mut first_send_done = ctx.first_send_done;
        let mut capabilities = Capabilities::relay(*platform);
        if !interactive {
            capabilities.buttons = "numbered_text".into();
        }
        let (parts, rendering) = RelayText.render(message, &capabilities);
        let mut result = IntentReceipt {
            message_ref: None,
            rendering,
            delivery: DeliveryOutcome::Rejected {
                reason: "empty_message".into(),
            },
        };
        let ReplyAddress::NyxidRelay {
            route_id,
            reply_token,
            reply_token_expires_at,
            ..
        } = &event.reply
        else {
            result.delivery = DeliveryOutcome::Unsupported;
            return (result, SendEffects { first_send_done });
        };
        let _ticket = match ctx.gate.enter_route(route_id).await {
            Ok(ticket) => ticket,
            Err(refusal) => {
                result.delivery = refusal.outcome();
                return (result, SendEffects { first_send_done });
            }
        };
        let mut confirmed_parts = 0;
        let mut first_platform_id = None;
        for (index, part) in parts.iter().enumerate() {
            let Some(mut slot) = self.pacing.acquire(route_id).await else {
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "pacing_capacity".into(),
                };
                break;
            };
            let permit = match ctx.gate.attempt().await {
                Ok(permit) => permit,
                Err(refusal) => {
                    result.delivery = refusal.outcome();
                    break;
                }
            };
            // Commit point (Amendment R3 §R3.1): the first part's request is
            // this send's first side effect.
            if index == 0 && !ctx.gate.commit() {
                result.delivery = expired();
                break;
            }
            // Choose immediately before dispatch, after pacing, because the token may expire while queued.
            let use_token = !first_send_done
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
                result.delivery = DeliveryOutcome::Rejected {
                    reason: "no_credential".into(),
                };
                break;
            };
            first_send_done = true;
            let mut outcome = self.relay.send(&event.reply, credential, &part.text).await;
            drop(permit);
            if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                slot.cooldown(retry_after);
                slot.retry().await;
                let retry = match ctx.gate.attempt().await {
                    Ok(permit) => permit,
                    Err(refusal) => {
                        result.delivery = refusal.outcome();
                        break;
                    }
                };
                // The single-use credential is never presented twice, even after a 429.
                outcome = match ctx.channel.definition.secrets.channel_key.as_ref() {
                    Some(key) => {
                        self.relay
                            .send(&event.reply, Credential::ChannelKey(key), &part.text)
                            .await
                    }
                    None => DeliveryOutcome::Rejected {
                        reason: "no_credential".into(),
                    },
                };
                drop(retry);
                if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                    slot.cooldown(retry_after);
                    outcome = unknown("rate_limited_after_retry");
                }
            }
            ctx.gate.progress();
            tracing::info!(
                outcome = outcome.kind(),
                part = index,
                "relay delivery finished"
            );
            if let DeliveryOutcome::Confirmed {
                platform_message_id: Some(id),
            } = &outcome
            {
                if result.message_ref.is_none() {
                    first_platform_id = Some(id.clone());
                    result.message_ref = MessageRef::seal(
                        &MessageRefPayload {
                            channel_id: event.channel_id.clone(),
                            event_id: event.event_id.clone(),
                            platform_message_id: id.clone(),
                            route_id: route_id.clone(),
                            interaction_record_id: None,
                            consumer: event.consumer.clone(),
                        },
                        &self.keys,
                    )
                    .ok();
                }
            }
            let confirmed = matches!(outcome, DeliveryOutcome::Confirmed { .. });
            result.delivery = outcome;
            if !confirmed {
                break;
            }
            confirmed_parts += 1;
        }
        if confirmed_parts > 0 {
            if confirmed_parts == parts.len() {
                result.delivery = DeliveryOutcome::Confirmed {
                    platform_message_id: first_platform_id,
                };
            } else {
                result.delivery = unknown("partial_delivery");
            }
        }
        (result, SendEffects { first_send_done })
    }
    fn claim_token(&self, token: &str, expires: i64) -> bool {
        let digest = Sha256::digest(token.as_bytes()).into();
        let mut consumed = self.consumed.lock().unwrap();
        consumed.retain(|_, expiry| *expiry > now());
        if consumed.len() >= 50_000 || consumed.contains_key(&digest) {
            return false;
        }
        consumed.insert(digest, expires);
        true
    }
}
/// A receipt for a send a gate stopped before its side effect.
pub(super) fn refused(rendering: RenderingReport, delivery: DeliveryOutcome) -> IntentReceipt {
    IntentReceipt {
        message_ref: None,
        rendering,
        delivery,
    }
}
pub(super) fn unsupported() -> IntentReceipt {
    IntentReceipt {
        message_ref: None,
        rendering: RenderingReport {
            outcome: RenderingOutcome::Unsupported,
            blocks: vec![],
        },
        delivery: DeliveryOutcome::Unsupported,
    }
}

/// Whether a card sent on `route_id` can be replaced. Only Telegram relays
/// prepare cards, so the route's own relay must be a Telegram one. The message
/// reference carries the relay route its card was sent on (`route_id`, copied
/// from the sealed reply address when the card was bound), so no reply address
/// is needed. With at most one relay the channel-level check is kept verbatim;
/// the two agree on every legacy definition, whose only relay is the route's.
fn replaces_telegram_cards(definition: &ChannelDefinition, route_id: &str) -> bool {
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

impl Sender {
    async fn interactive_reply(
        &self,
        ctx: &SendContext<'_>,
        event: &EventRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        let mut effects = SendEffects {
            first_send_done: ctx.first_send_done,
        };
        let (parts, rendering) = RelayText.render(message, &Capabilities::telegram());
        let rejected = |reason: &str| IntentReceipt {
            message_ref: None,
            rendering: rendering.clone(),
            delivery: DeliveryOutcome::Rejected {
                reason: reason.into(),
            },
        };
        if parts.len() != 1 {
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
        let Some(_slot) = self.pacing.acquire(route_id).await else {
            return (rejected("pacing_capacity"), effects);
        };
        let _permit = match ctx.gate.attempt().await {
            Ok(permit) => permit,
            Err(refusal) => return (refused(rendering, refusal.outcome()), effects),
        };
        effects.first_send_done = true;
        let (delivery, outbound) = self
            .relay
            .send_rich(
                &event.reply,
                Credential::ChannelKey(key),
                &parts[0].text,
                Some(&prepared.markup),
            )
            .await;
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
    pub async fn update(
        &self,
        ctx: &SendContext<'_>,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> (IntentReceipt, SendEffects) {
        // A replacement answers no event, so it never marks a first send.
        let effects = SendEffects::default();
        let channel = ctx.channel;
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
        if reference.interaction_record_id.is_none()
            || !replaces_telegram_cards(&channel.definition, &reference.route_id)
        {
            return (unsupported(), effects);
        }
        let (parts, rendering) = RelayText.render(message, &Capabilities::telegram());
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
        let mut prepared = match store.replacement(channel, reference, message).await {
            Ok(p) => p,
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
        let delivery = self
            .relay
            .edit(key, &destination, &parts[0].text, &prepared.markup)
            .await;
        let mut result = IntentReceipt {
            message_ref: None,
            rendering,
            delivery,
        };
        if matches!(result.delivery, DeliveryOutcome::Confirmed { .. }) {
            match store.bind(&mut prepared, destination).await {
                Ok(reference) => result.message_ref = Some(reference),
                Err(_) => result.delivery = unknown("interaction_receipt_unconfirmed"),
            }
        }
        (result, effects)
    }
}
