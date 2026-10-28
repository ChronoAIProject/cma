//! Explicit, durable effects on confirmed native bot messages.
pub(crate) mod authority;
pub(crate) mod discord;
mod discord_card;
#[cfg(test)]
mod discord_card_failures;
#[cfg(test)]
pub(crate) mod discord_card_fixture;
#[cfg(test)]
mod discord_card_races;
#[cfg(test)]
mod discord_card_tests;
#[cfg(test)]
mod discord_tests;
mod lark;
pub(crate) mod lark_authority;
pub(crate) mod lark_ledger;
pub mod lark_transport;
pub(crate) mod mutation;
pub(crate) mod native_lark;
pub(crate) mod native_lark_authority;
mod native_lark_card;
pub(crate) mod native_lark_context;
mod native_lark_execute;
mod received;
mod received_authority;
mod received_transport;
pub(crate) mod whatsapp;
pub(crate) mod whatsapp_authority;
use super::{reply_targets::receipts, state::owner, ToolsState};
use crate::{
    auth::AgentCaller,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    telegram::{Error, TelegramClient},
};
use cmaeg_core::{
    ids::MessageRef,
    message::{RenderingOutcome, RenderingReport},
    message_effect::MessageReactionRequest,
    problem::{Problem, ProblemCode},
    seal::MessageRefPayload,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Semaphore;
use utoipa::ToSchema;

pub struct MessageEffectsClient {
    telegram: Arc<TelegramClient>,
    capacity: Semaphore,
    received_issuance_enabled: bool,
}
impl MessageEffectsClient {
    pub fn new(telegram: Arc<TelegramClient>) -> Self {
        Self {
            telegram,
            capacity: Semaphore::new(32),
            received_issuance_enabled: false,
        }
    }
    pub fn with_received_reactions(mut self, enabled: bool) -> Self {
        self.received_issuance_enabled = enabled;
        self
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageEffectsCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_edit: Option<crate::native_media::Capabilities>,
    pub delete: bool,
    pub reaction: bool,
    /// Exact bot-message or received-user-message authority; never arbitrary IDs.
    pub scope: String,
    pub max_reactions: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}
impl MessageEffectsCapabilities {
    pub(crate) fn new(supported: bool, expires_at: Option<i64>) -> Self {
        Self {
            media_edit: None,
            delete: supported,
            reaction: supported,
            scope: "confirmed_native_bot_message".into(),
            max_reactions: if supported { 1 } else { 0 },
            expires_at,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct MessageEffectReceipt {
    pub delivery: DeliveryOutcome,
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Effect {
    Delete,
    Reaction { request: MessageReactionRequest },
}
impl ToolsState {
    pub async fn message_effect_capabilities(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
    ) -> Result<MessageEffectsCapabilities, Problem> {
        let (channel, message) = self.current_message(caller, reference).await?;
        if message.native_media_edit.is_some() {
            return self.native_media_capabilities(caller, reference).await;
        }
        if message.received_reaction.is_some() {
            return received::capabilities(
                &message,
                &channel,
                caller,
                self.received_reaction_client(&message),
            );
        }
        if crate::interactions::native_lark::is_reference(&message) {
            return self.native_lark_card_capabilities(&channel, &message).await;
        }
        if message.native_lark_effect.is_some() {
            native_lark_authority::retained(self, &message).await?;
            return native_lark::capabilities(
                &message,
                &channel,
                caller,
                self.native_lark_effects.is_some(),
            );
        }
        if message.whatsapp_reaction.is_some() {
            return whatsapp::capabilities(
                &message,
                &channel,
                caller,
                self.whatsapp_reactions.is_some()
                    && self
                        .resolver
                        .record_client()
                        .whatsapp_reaction_storage_available(),
            );
        }
        if message.interaction_record_id.is_some() && message.route_id.starts_with("discord:") {
            return self
                .discord_card_capabilities(caller, reference, channel, message)
                .await;
        }
        if message.lark_effect.is_some() {
            lark_authority::check(&message, &channel, caller)?;
            let cards = crate::interactions::InteractionStore::new(
                self.resolver.record_client(),
                self.keys.clone(),
                self.providers.clone(),
            );
            if cards
                .lark_effect_card(&channel, &message)
                .await
                .map_err(|_| invalid("lark_card_unavailable"))?
                .is_some_and(|(card, _)| card.superseded)
            {
                return Ok(MessageEffectsCapabilities::new(
                    false,
                    message.lark_effect.as_ref().map(|a| a.expires_at),
                ));
            }
            return lark::capabilities(&message, &channel, caller, self.lark_effects.is_some());
        }
        if message.discord_message.is_some() {
            return discord::capabilities(&message, &channel, self.discord_effects.is_some());
        }
        let Some(authority) = &message.telegram_effect else {
            return Ok(MessageEffectsCapabilities::new(false, None));
        };
        authority::check(&message, &channel, caller, now())?;
        Ok(MessageEffectsCapabilities::new(
            self.telegram_effects.is_some(),
            Some(authority.expires_at),
        ))
    }
    pub async fn delete_message(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
    ) -> Result<MessageEffectReceipt, Problem> {
        self.message_effect(caller, reference, request_id, Effect::Delete)
            .await
    }
    pub async fn set_message_reaction(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        request: MessageReactionRequest,
    ) -> Result<MessageEffectReceipt, Problem> {
        request
            .validate()
            .map_err(|_| invalid("invalid reaction"))?;
        self.message_effect(caller, reference, request_id, Effect::Reaction { request })
            .await
    }
    async fn current_message(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
    ) -> Result<
        (
            Arc<crate::channels::resolver::ResolvedChannel>,
            MessageRefPayload,
        ),
        Problem,
    > {
        let message = reference
            .open(&self.keys)
            .map_err(|_| invalid("message_ref_invalid"))?
            .payload;
        let channel = self
            .resolver
            .resolve_current(message.channel_id.as_str())
            .await
            .map_err(|e| e.problem())?;
        owner(caller, &channel, message.consumer.as_ref(), "messageEffect")?;
        Ok((channel, message))
    }
    async fn message_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
    ) -> Result<MessageEffectReceipt, Problem> {
        if request_id.is_empty()
            || request_id.len() > 128
            || !request_id.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(invalid(
                "a 1..128 byte printable Idempotency-Key is required",
            ));
        }
        let (channel, message) = self.current_message(caller, reference).await?;
        if message.received_reaction.is_some() {
            return self
                .received_message_reaction(caller, reference, request_id, effect, channel, message)
                .await;
        }
        if crate::interactions::native_lark::is_reference(&message) {
            return self
                .native_lark_card_effect(caller, reference, request_id, effect)
                .await;
        }
        if message.native_lark_effect.is_some() {
            return self
                .native_lark_message_effect(caller, reference, request_id, effect, channel, message)
                .await;
        }
        if message.whatsapp_reaction.is_some() {
            return self
                .whatsapp_message_reaction(caller, reference, request_id, effect, channel, message)
                .await;
        }
        if message.interaction_record_id.is_some() && message.route_id.starts_with("discord:") {
            return self
                .discord_card_effect(caller, reference, request_id, effect, channel, message)
                .await;
        }
        if message.discord_message.is_some() {
            return self
                .discord_message_effect(caller, reference, request_id, effect, channel, message)
                .await;
        }
        if message.lark_effect.is_some() {
            return self
                .lark_message_effect(caller, reference, request_id, effect, channel, message)
                .await;
        }
        let (Some(authority), Some(client)) = (&message.telegram_effect, &self.telegram_effects)
        else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        authority::check(&message, &channel, caller, now())?;
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        // The sealed payload, never randomized ciphertext, owns the durable key.
        // Both effect types share one scope, so reusing a key for a different action conflicts.
        let claim = match store.claim_scoped("telegram_message_effect_v1", "cmaeg.telegram-effect.v1",
            receipts::hash(&serde_json::json!({"channel":message.channel_id,"consumer":message.consumer,"authority":authority,"message":message.platform_message_id})), request_id, receipts::hash(&effect),
            &channel.definition.owner.subject, authority.expires_at).await? {
            receipts::Admission::Replay(receipt) => return Ok(MessageEffectReceipt { delivery: receipt.delivery }),
            receipts::Admission::Claimed(claim) => claim,
        };
        // New media scopes also serialize different keys. Historical text scopes
        // retain their existing receipt behavior and serialized authority.
        let serial =
            mutation::Store::telegram_media(self.resolver.record_client(), self.keys.clone());
        let held = if authority.effects_only {
            match serial
                .acquire(
                    mutation::destination(
                        &authority.bot_id,
                        &authority.chat_id,
                        &message.platform_message_id,
                    ),
                    &channel.definition.owner.subject,
                    authority.expires_at,
                )
                .await?
            {
                Some(held) => Some(held),
                None => {
                    let mut receipt = receipts::uncertain("effect_receipt_unconfirmed");
                    receipt.delivery = DeliveryOutcome::Rejected {
                        reason: "native_message_effect_busy".into(),
                    };
                    store.finish(claim, &receipt).await?;
                    return Ok(MessageEffectReceipt {
                        delivery: receipt.delivery,
                    });
                }
            }
        } else {
            None
        };
        // Recheck live ownership, current source/actor policy and consumer after CAS/fence.
        let (channel, message) = self.current_message(caller, reference).await?;
        authority::check(&message, &channel, caller, now())?;
        let authority = message
            .telegram_effect
            .as_ref()
            .expect("checked native authority");
        let token = channel
            .definition
            .secrets
            .telegram_bot_token
            .as_ref()
            .expect("checked native token");
        let sent = match effect {
            Effect::Delete => {
                client
                    .telegram
                    .delete_message(token, &authority.chat_id, &message.platform_message_id)
                    .await
            }
            Effect::Reaction { request } => {
                client
                    .telegram
                    .set_message_reaction(
                        token,
                        &authority.chat_id,
                        &message.platform_message_id,
                        &request,
                    )
                    .await
            }
        };
        let delivery = match sent {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(message.platform_message_id),
            },
            Err(Error::Rejected) => DeliveryOutcome::Rejected {
                reason: "telegram_rejected".into(),
            },
            Err(Error::RateLimited(retry_after)) => DeliveryOutcome::RateLimited { retry_after },
            Err(Error::Unconfirmed) => DeliveryOutcome::Unknown {
                reason: "telegram_response_unconfirmed".into(),
            },
        };
        let receipt = IntentReceipt {
            message_parts: vec![],
            message_ref: None,
            rendering: RenderingReport {
                outcome: RenderingOutcome::Unsupported,
                blocks: vec![],
            },
            delivery,
        };
        // A lost final write cannot turn an admitted external effect into a safe retry.
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unknown {
                    reason: "effect_receipt_unconfirmed".into(),
                },
            });
        }
        if let Some(held) =
            held.filter(|_| !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }))
        {
            if serial.release(held).await.is_err() {
                tracing::warn!(
                    "native message effect fence remains held after a completed receipt"
                );
            }
        }
        Ok(MessageEffectReceipt {
            delivery: receipt.delivery,
        })
    }
}
fn invalid(detail: &str) -> Problem {
    Problem::new(ProblemCode::InvalidRequest, 400, detail)
}
fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

#[cfg(test)]
pub(crate) mod lark_fixture;
#[cfg(test)]
mod lark_tests;

#[cfg(test)]
mod lark_card_tests;

#[cfg(test)]
mod lark_custody_tests;

#[cfg(test)]
mod native_lark_fixture;
#[cfg(test)]
mod native_lark_race_tests;
#[cfg(test)]
mod native_lark_tests;
#[cfg(test)]
mod whatsapp_tests;

#[cfg(test)]
mod native_lark_card_tests;
#[cfg(test)]
mod received_authority_tests;
#[cfg(test)]
mod received_fixture;
#[cfg(test)]
mod received_retry_tests;
#[cfg(test)]
mod received_tests;
