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
    channel::Platform,
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
    /// Report the consumer bound to this event reference, at its origin.
    pub async fn observe(
        &self,
        event: &EventRefPayload,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
    ) {
        self.observe_at(event, step, outcome, Destination::Origin, None)
            .await;
    }
    pub async fn observe_at(
        &self,
        event: &EventRefPayload,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        destination: Destination,
        default: Option<&super::destination::DefaultDestination>,
    ) {
        self.observer.delivered(
            &event.record_id,
            &event.event_id,
            event
                .consumer
                .as_ref()
                .unwrap_or_else(|| default_consumer()),
            step,
            outcome,
            destination,
        );
        if default.is_none()
            && matches!(
                event.reply,
                ReplyAddress::Sync { .. } | ReplyAddress::ResultWebhook { .. } | ReplyAddress::None
            )
        {
            // Custom delivery reports its actual local queue and each webhook
            // leg separately; the compatibility aggregate is not another send.
            return;
        }
        self.observer
            .checkpoint(
                &event.record_id,
                &event.event_id,
                crate::activity::facts::delivery(
                    event
                        .consumer
                        .as_ref()
                        .unwrap_or_else(|| default_consumer())
                        .as_str(),
                    step,
                    outcome,
                    destination,
                    &event.reply,
                    default,
                ),
            )
            .await;
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
                ReplyAddress::Whatsapp { .. } | ReplyAddress::WhatsappGroup(_) => "whatsapp",
                ReplyAddress::Telegram { .. } => "telegram",
                ReplyAddress::Discord { .. } => "discord",
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
        if message.native_media_update.is_some() {
            return (unsupported(), SendEffects::default());
        }
        if matches!(event.reply, ReplyAddress::NativeLark(_)) {
            return self.native_lark_reply(ctx, event, message).await;
        }
        if matches!(
            event.reply,
            ReplyAddress::Whatsapp { .. } | ReplyAddress::WhatsappGroup(_)
        ) {
            return self.whatsapp_reply(ctx, event, message).await;
        }
        if message
            .native_attachments
            .iter()
            .any(|item| item.kind == cmaeg_core::message::NativeMediaKind::Sticker)
        {
            return (unsupported(), SendEffects::default());
        }
        if matches!(event.reply, ReplyAddress::Discord { .. }) {
            return self.discord_reply(ctx, event, message).await;
        }
        if !message.native_attachments.is_empty()
            && matches!(event.reply, ReplyAddress::Telegram { .. })
        {
            return self.native_media_reply(ctx, event, message).await;
        }
        if message.whatsapp_template.is_some() {
            if !self.relay.whatsapp_templates_enabled()
                || message.validate().is_err()
                || !matches!(
                    event.reply,
                    ReplyAddress::NyxidRelay {
                        platform: Platform::Whatsapp,
                        ..
                    }
                )
            {
                return (unsupported(), SendEffects::default());
            }
            return self.complete_reply(ctx, event, message, false).await;
        }
        if !message.native_attachments.is_empty()
            && (!self.relay.native_media_enabled()
                || !matches!(
                    event.reply,
                    ReplyAddress::NyxidRelay {
                        platform: Platform::Telegram
                            | Platform::Lark
                            | Platform::Feishu
                            | Platform::Discord
                            | Platform::Whatsapp,
                        ..
                    }
                )
                || crate::interactions::has_controls(message))
        {
            return (unsupported(), SendEffects::default());
        }
        if message.thread_reply
            && !matches!(
                event.reply,
                ReplyAddress::NyxidRelay {
                    thread_reply: true,
                    ..
                }
            )
        {
            let mut result = unsupported();
            result.delivery = DeliveryOutcome::Rejected {
                reason: "thread_target_unavailable".into(),
            };
            return (result, SendEffects::default());
        }
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
        let native_controls = (self.relay.lark_cards_enabled()
            && crate::interactions::lark::supported(*platform)
            && (crate::interactions::lark::requested(message) || self.native_lark_form(message)))
            || (*platform == Platform::Whatsapp
                && self.relay.whatsapp_interactions_enabled()
                && crate::interactions::whatsapp::requested(message));
        if native_controls {
            match self.relay_control_context(ctx, event).await {
                Ok(Some(context)) => {
                    return self
                        .interactive_reply(ctx, event, message, Some(context))
                        .await;
                }
                Ok(None) => {} // Preserve legacy text for contexts without native controls.
                Err(_) => return (unsupported(), SendEffects::default()),
            }
        }
        let interactive = *platform == Platform::Telegram && self.relay.interactions.is_some();
        if interactive && crate::interactions::has_controls(message) {
            return self.interactive_reply(ctx, event, message, None).await;
        }
        if self.relay.complete_replies() || !message.native_attachments.is_empty() {
            return self.complete_reply(ctx, event, message, interactive).await;
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
        let mut address = event.reply.clone();
        if let ReplyAddress::NyxidRelay { thread_reply, .. } = &mut address {
            *thread_reply = message.thread_reply;
        }
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
            let (mut outcome, mut outbound) = self
                .relay
                .send_rich(&address, credential, &part.text, None)
                .await;
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
                (outcome, outbound) = match ctx.channel.definition.secrets.channel_key.as_ref() {
                    Some(key) => {
                        self.relay
                            .send_rich(&address, Credential::ChannelKey(key), &part.text, None)
                            .await
                    }
                    None => (
                        DeliveryOutcome::Rejected {
                            reason: "no_credential".into(),
                        },
                        None,
                    ),
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
                            discord_message: None,
                            native_media_edit: None,
                            received_reaction: None,
                            native_lark_effect: None,
                            whatsapp_reaction: None,
                            lark_effect: None,
                            telegram_effect: None,
                            channel_id: event.channel_id.clone(),
                            event_id: event.event_id.clone(),
                            platform_message_id: id.clone(),
                            route_id: route_id.clone(),
                            interaction_record_id: None,
                            relay_message: (self.relay.text_edits_enabled() && parts.len() == 1)
                                .then(|| {
                                    super::edit::destination(
                                        *platform,
                                        message,
                                        &part.text,
                                        outbound.clone(),
                                    )
                                })
                                .flatten(),
                            consumer: event.consumer.clone(),
                        },
                        &self.keys,
                    )
                    .ok();
                }
            }
            if parts.len() == 1
                && !message.thread_reply
                && super::edit::destination(*platform, message, &part.text, outbound.clone())
                    .is_some()
            {
                if let (Some(reference), Some(outbound)) =
                    (result.message_ref.as_ref(), outbound.as_deref())
                {
                    result.message_ref = Some(
                        self.lark_effect_reference(ctx.channel, event, reference.clone(), outbound)
                            .await,
                    );
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
    pub(super) fn claim_token(&self, token: &str, expires: i64) -> bool {
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
