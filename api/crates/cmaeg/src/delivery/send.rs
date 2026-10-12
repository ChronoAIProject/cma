use super::{
    event::{now, EventState},
    intents::{DeliveryOutcome, IntentReceipt},
    pacing::Pacing,
    relay_client::{unknown, Credential, RelayClient},
    render::{RelayText, Renderer},
};
use crate::tools::Capabilities;
use cmaeg_core::{
    channel::Platform,
    ids::MessageRef,
    message::{OutboundMessage, RenderingOutcome, RenderingReport},
    seal::{MasterKeyRing, MessageRefPayload, ReplyAddress},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
pub(super) struct Sender {
    pub custom: super::custom::CustomDelivery,
    pub keys: Arc<MasterKeyRing>,
    pub relay: RelayClient,
    pub pacing: Pacing,
    // Token-use tombstones outlive the short event state but retain no credentials.
    pub consumed: Mutex<HashMap<[u8; 32], i64>>,
}
impl Sender {
    #[tracing::instrument(name="cmaeg.deliver", parent = &state.trace, skip_all, fields(
        channel = %crate::telemetry::channel_hash(state.channel.id.as_str()), event_id = %state.event.event_id))]
    pub async fn reply(&self, state: &mut EventState, message: &OutboundMessage) -> IntentReceipt {
        let mut observation = crate::telemetry::operations::Operation::new(
            crate::telemetry::names::DELIVERY_DURATION,
            crate::telemetry::names::PLATFORM,
            match &state.event.reply {
                ReplyAddress::NyxidRelay { platform, .. } => platform.as_str(),
                ReplyAddress::Telegram { .. } => "telegram",
                _ => "custom",
            },
        );
        let result = self.reply_inner(state, message).await;
        observation.finish(result.delivery.kind());
        result
    }
    async fn reply_inner(
        &self,
        state: &mut EventState,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        if matches!(state.event.reply, ReplyAddress::Telegram { .. }) {
            return self.native_reply(state, message).await;
        }
        let ReplyAddress::NyxidRelay { platform, .. } = &state.event.reply else {
            return self
                .custom
                .reply(&state.channel, &state.event, message)
                .await;
        };
        // Native controls and their updates are proven only for Telegram; every
        // other platform receives text through NyxID's generic relay reply.
        let interactive = *platform == Platform::Telegram && self.relay.interactions.is_some();
        if interactive && crate::interactions::has_controls(message) {
            return self.interactive_reply(state, message).await;
        }
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
        } = &state.event.reply
        else {
            result.delivery = DeliveryOutcome::Unsupported;
            return result;
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
            // Choose immediately before dispatch, after pacing, because the token may expire while queued.
            let use_token = !state.first_send_done
                && *reply_token_expires_at > now()
                && self.claim_token(reply_token.as_str(), *reply_token_expires_at);
            let credential = if use_token {
                Some(Credential::ReplyToken(reply_token))
            } else {
                state
                    .channel
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
            state.first_send_done = true;
            let mut outcome = self
                .relay
                .send(&state.event.reply, credential, &part.text)
                .await;
            if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                slot.cooldown(retry_after);
                slot.retry().await;
                // The single-use credential is never presented twice, even after a 429.
                outcome = match state.channel.definition.secrets.channel_key.as_ref() {
                    Some(key) => {
                        self.relay
                            .send(&state.event.reply, Credential::ChannelKey(key), &part.text)
                            .await
                    }
                    None => DeliveryOutcome::Rejected {
                        reason: "no_credential".into(),
                    },
                };
                if let DeliveryOutcome::RateLimited { retry_after } = outcome {
                    slot.cooldown(retry_after);
                    outcome = unknown("rate_limited_after_retry");
                }
            }
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
                            channel_id: state.event.channel_id.clone(),
                            event_id: state.event.event_id.clone(),
                            platform_message_id: id.clone(),
                            route_id: route_id.clone(),
                            interaction_record_id: None,
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
        result
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

impl Sender {
    async fn interactive_reply(
        &self,
        state: &mut EventState,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        let (parts, rendering) = RelayText.render(message, &Capabilities::telegram());
        let rejected = |reason: &str| IntentReceipt {
            message_ref: None,
            rendering: rendering.clone(),
            delivery: DeliveryOutcome::Rejected {
                reason: reason.into(),
            },
        };
        if parts.len() != 1 {
            return rejected("interactive_message_must_fit_one_part");
        }
        let Some(key) = state.channel.definition.secrets.channel_key.as_ref() else {
            return rejected("no_channel_credential");
        };
        let Some(store) = &self.relay.interactions else {
            return rejected("interactions_unavailable");
        };
        let mut prepared = match store.prepare(&state.channel, &state.event, message).await {
            Ok(p) => p,
            Err(_) => return rejected("interaction_state_unavailable"),
        };
        let cmaeg_core::seal::ReplyAddress::NyxidRelay { route_id, .. } = &state.event.reply else {
            return unsupported();
        };
        let Some(_slot) = self.pacing.acquire(route_id).await else {
            return rejected("pacing_capacity");
        };
        state.first_send_done = true;
        let (delivery, outbound) = self
            .relay
            .send_rich(
                &state.event.reply,
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
        result
    }
    pub async fn update(
        &self,
        channel: &crate::channels::resolver::ResolvedChannel,
        reference: &MessageRefPayload,
        message: &OutboundMessage,
    ) -> IntentReceipt {
        if reference.route_id.starts_with("telegram:")
            && channel
                .definition
                .sources
                .iter()
                .any(|source| matches!(source, cmaeg_core::channel::Source::Telegram { .. }))
        {
            return self.native_update(channel, reference, message).await;
        }
        let Some(store) = &self.relay.interactions else {
            return unsupported();
        };
        if reference.interaction_record_id.is_none()
            || channel.definition.relay_platform() != Some(Platform::Telegram)
        {
            return unsupported();
        }
        let (parts, rendering) = RelayText.render(message, &Capabilities::telegram());
        if parts.len() != 1 {
            return IntentReceipt {
                message_ref: None,
                rendering,
                delivery: DeliveryOutcome::Rejected {
                    reason: "interactive_message_must_fit_one_part".into(),
                },
            };
        }
        let Some(key) = channel.definition.secrets.channel_key.as_ref() else {
            return unsupported();
        };
        let mut prepared = match store.replacement(channel, reference, message).await {
            Ok(p) => p,
            Err(_) => {
                return IntentReceipt {
                    message_ref: None,
                    rendering,
                    delivery: DeliveryOutcome::Rejected {
                        reason: "interaction_expired_or_invalid".into(),
                    },
                }
            }
        };
        let Some(destination) = prepared.card.destination.clone() else {
            return unsupported();
        };
        let Some(_slot) = self.pacing.acquire(&reference.route_id).await else {
            return unsupported();
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
        result
    }
}
