//! Reaction-only tools for exact original incoming Telegram and Discord messages.
use super::{
    mutation, receipts, received_authority, Effect, MessageEffectReceipt,
    MessageEffectsCapabilities, ToolsState,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{
        received_reaction::{ReceivedReactionPlatform as Platform, MAX_AGE},
        EventRefPayload, MessageRefPayload,
    },
};
use std::sync::Arc;

pub(super) fn capabilities(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
    client: bool,
) -> Result<MessageEffectsCapabilities, Problem> {
    received_authority::check(message, channel, caller)?;
    Ok(MessageEffectsCapabilities {
        media_edit: None,
        delete: false,
        reaction: client,
        scope: "received_native_user_message".into(),
        max_reactions: u8::from(client),
        expires_at: message.received_reaction.as_ref().map(|a| a.expires_at),
    })
}

impl ToolsState {
    pub(super) fn received_reaction_client(&self, message: &MessageRefPayload) -> bool {
        match message.received_reaction.as_ref().map(|a| a.platform) {
            Some(Platform::Telegram) => self.telegram_effects.is_some(),
            Some(Platform::Discord) => self.discord_effects.is_some(),
            None => false,
        }
    }

    pub(super) async fn native_received_message_context(
        &self,
        caller: AgentCaller,
        event: &EventRefPayload,
        activity: &Activity,
        verified: bool,
    ) -> Option<super::whatsapp::ReceivedMessageContext> {
        let platform = match activity.source {
            SourceKind::Telegram { .. } => Platform::Telegram,
            SourceKind::Discord { .. } => Platform::Discord,
            _ => return None,
        };
        if !verified || !self.received_reaction_enabled(platform) {
            return None;
        }
        let channel = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            crate::turns::routed::current_received_message(
                self.resolver.record_client(),
                self.keys.clone(),
                &self.resolver,
                caller,
                event,
                activity,
            ),
        )
        .await
        .ok()?
        .ok()?;
        let message_ref = received_authority::issue(&channel, event, activity, &self.keys)?;
        let message = message_ref.open(&self.keys).ok()?.payload;
        let effects = capabilities(&message, &channel, caller, true).ok()?;
        Some(super::whatsapp::ReceivedMessageContext {
            message_ref,
            effects,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn received_message_reaction(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        received_authority::check(&message, &channel, caller)?;
        let Effect::Reaction { request } = &effect else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        if !self.received_reaction_client(&message) {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        }
        let a = message
            .received_reaction
            .as_ref()
            .expect("checked received scope");
        if a.platform == Platform::Discord && request.is_big {
            return Err(super::invalid("Discord does not support is_big"));
        }
        let capacity = match a.platform {
            Platform::Telegram => &self.telegram_effects.as_ref().unwrap().capacity,
            Platform::Discord => &self.discord_effects.as_ref().unwrap().capacity,
        };
        let _permit = capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        // Stable identity excludes randomized ciphertext, expiry and mutable credentials.
        // A new seal or consumer credential cannot repeat a previously claimed key.
        let scope = receipts::hash(&serde_json::json!({
            "channel":message.channel_id,"event":message.event_id,"consumer":message.consumer,
            "platform":a.platform,"bot":a.bot_id,"chat":a.chat_id,"message":message.platform_message_id,
        }));
        let retained_until = a.received_at.saturating_add(MAX_AGE);
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match store
            .claim_scoped(
                "received_reaction_v1",
                "cmaeg.received-reaction.v1",
                scope,
                request_id,
                receipts::hash(&effect),
                &channel.definition.owner.subject,
                retained_until,
            )
            .await?
        {
            receipts::Admission::Replay(receipt) => {
                return Ok(MessageEffectReceipt {
                    delivery: receipt.delivery,
                })
            }
            receipts::Admission::Claimed(claim) => claim,
        };
        let serial = match a.platform {
            Platform::Telegram => {
                mutation::Store::telegram_received(self.resolver.record_client(), self.keys.clone())
            }
            Platform::Discord => {
                mutation::Store::discord(self.resolver.record_client(), self.keys.clone())
            }
        };
        let held = serial
            .acquire(
                mutation::destination(&a.bot_id, &a.chat_id, &message.platform_message_id),
                &channel.definition.owner.subject,
                retained_until,
            )
            .await?;
        let Some(held) = held else {
            let receipt = receipts::uncertain("received_reaction_busy_or_uncertain");
            store.finish(claim, &receipt).await?;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        };
        let fresh = async {
            let (channel, message) = self.current_message(caller, reference).await?;
            received_authority::check(&message, &channel, caller)?;
            Ok::<_, Problem>((channel, message))
        }
        .await;
        let (fresh, message) = match fresh {
            Ok(current) => current,
            Err(problem) => {
                let mut receipt = receipts::uncertain("received_reaction_revoked");
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "received_reaction_revoked".into(),
                };
                store.finish(claim, &receipt).await?;
                let _ = serial.release(held).await;
                return Err(problem);
            }
        };
        let mut receipt = receipts::uncertain("received_reaction_unconfirmed");
        receipt.delivery = self
            .send_received_reaction(caller, reference, &fresh, &message, request)
            .await;
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unknown {
                    reason: "received_reaction_receipt_unconfirmed".into(),
                },
            });
        }
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        Ok(MessageEffectReceipt {
            delivery: receipt.delivery,
        })
    }
}
