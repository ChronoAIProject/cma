//! Durable effects for independently opted-in Lark/Feishu sources.
use super::{
    lark_authority, lark_ledger, lark_transport, receipts, Effect, MessageEffectReceipt,
    MessageEffectsCapabilities, ToolsState,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
};
use cmaeg_core::{
    ids::MessageRef,
    message_effect::TelegramReaction,
    problem::{Problem, ProblemCode},
    seal::MessageRefPayload,
};
use std::{sync::Arc, time::Duration};
use tokio::time::Instant;

pub(super) fn capabilities(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
    enabled: bool,
) -> Result<MessageEffectsCapabilities, Problem> {
    lark_authority::check(message, channel, caller)?;
    let a = message.lark_effect.as_ref().expect("checked authority");
    Ok(MessageEffectsCapabilities {
        media_edit: None,
        delete: enabled && a.binding.delete,
        reaction: enabled && a.binding.reaction,
        scope: "confirmed_native_bot_message".into(),
        max_reactions: u8::from(enabled && a.binding.reaction),
        expires_at: Some(a.expires_at),
    })
}
impl ToolsState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn lark_message_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        let Some(client) = &self.lark_effects else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        lark_authority::check(&message, &channel, caller)?;
        let a = message.lark_effect.as_ref().expect("checked authority");
        let desired = match &effect {
            Effect::Delete if a.binding.delete => None,
            Effect::Reaction { request } if a.binding.reaction => {
                if request.is_big {
                    return Err(super::invalid("Lark does not support is_big"));
                }
                match request.reaction.first() {
                    None => None,
                    Some(TelegramReaction::Emoji { emoji })
                        if !emoji.is_empty()
                            && emoji.len() <= 64
                            && emoji
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
                    {
                        Some(emoji.as_str())
                    }
                    _ => {
                        return Err(super::invalid(
                            "Lark requires a platform emoji_type such as THUMBSUP",
                        ))
                    }
                }
            }
            _ => {
                return Ok(MessageEffectReceipt {
                    delivery: DeliveryOutcome::Unsupported,
                })
            }
        };
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match store.claim_scoped("lark_message_effect_v1", "cmaeg.lark-effect.v1",
            receipts::hash(&serde_json::json!({"channel":message.channel_id,"consumer":message.consumer,"authority":a,"message":message.platform_message_id})),
            request_id, receipts::hash(&effect), &channel.definition.owner.subject, a.expires_at).await? {
            receipts::Admission::Replay(receipt) => return Ok(MessageEffectReceipt { delivery: receipt.delivery }),
            receipts::Admission::Claimed(claim) => claim,
        };
        let serial = lark_ledger::Store::new(self.resolver.record_client(), self.keys.clone());
        let held = serial
            .acquire(
                lark_ledger::destination(&message),
                &channel.definition.owner.subject,
                a.expires_at,
            )
            .await?;
        let Some(mut held) = held else {
            let receipt = receipts::uncertain("lark_message_effect_busy_or_uncertain");
            store.finish(claim, &receipt).await?;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        };
        let (channel, message) = self.current_message(caller, reference).await?;
        lark_authority::check(&message, &channel, caller)?;
        let cards = crate::interactions::InteractionStore::new(
            self.resolver.record_client(),
            self.keys.clone(),
            self.providers.clone(),
        );
        let card = cards
            .lark_effect_card(&channel, &message)
            .await
            .map_err(|_| super::invalid("lark_card_unavailable"))?;
        if card.as_ref().is_some_and(|(card, _)| card.superseded) {
            let mut receipt = receipts::uncertain("lark_card_superseded");
            receipt.delivery = DeliveryOutcome::Unsupported;
            store.finish(claim, &receipt).await?;
            let _ = serial.release(held).await;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        }
        let claim_id = claim.id().to_owned();
        let deleting = matches!(effect, Effect::Delete);
        if deleting
            && cards
                .begin_lark_delete(&channel, &message, &claim_id)
                .await
                .is_err()
        {
            return Ok(MessageEffectReceipt {
                delivery: lark_transport::unknown(),
            });
        }
        let key = channel
            .definition
            .secrets
            .channel_key
            .as_ref()
            .expect("checked channel key");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut next = held.reaction.clone();
        let sent = match client.authorize(key, &message, deadline).await {
            Err(error) => Err(error),
            Ok(()) => {
                // Metadata/network reads can take time; recheck local custody at the mutation boundary.
                let (fresh, current) = self.current_message(caller, reference).await?;
                lark_authority::check(&current, &fresh, caller)?;
                match &effect {
                    Effect::Delete => client.delete(key, &message, deadline).await,
                    Effect::Reaction { .. } => match client
                        .reaction(key, &message, held.reaction.as_ref(), desired, deadline)
                        .await
                    {
                        Ok(reaction) => {
                            next = reaction;
                            Ok(())
                        }
                        Err(error) => Err(error),
                    },
                }
            }
        };
        let mut receipt = receipts::uncertain("lark_effect_unconfirmed");
        receipt.delivery = match sent {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(message.platform_message_id.clone()),
            },
            Err(error) => error,
        };
        if deleting
            && cards
                .finish_lark_delete(&message, &claim_id, &receipt.delivery)
                .await
                .is_err()
        {
            return Ok(MessageEffectReceipt {
                delivery: lark_transport::unknown(),
            });
        }
        if matches!(receipt.delivery, DeliveryOutcome::Confirmed { .. })
            && serial.retain(&mut held, next).await.is_err()
        {
            return Ok(MessageEffectReceipt {
                delivery: lark_transport::unknown(),
            });
        }
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: lark_transport::unknown(),
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
