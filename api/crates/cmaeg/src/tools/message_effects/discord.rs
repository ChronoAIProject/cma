//! Retained exact-message authority and durable claims for native Discord effects.
use super::{receipts, Effect, MessageEffectReceipt, MessageEffectsCapabilities, ToolsState};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::{discord_edit, intents::DeliveryOutcome},
    discord::{rest::token, DiscordRest, RestError},
};
use cmaeg_core::{
    activity::{Activity, SourceKind},
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{DiscordEffectWindow, EventRefPayload, MessageRefPayload, ReplyAddress},
};
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct DiscordEffectsClient {
    api: DiscordRest,
    capacity: Semaphore,
}
impl DiscordEffectsClient {
    pub fn new(api: DiscordRest) -> Self {
        Self {
            api,
            capacity: Semaphore::new(32),
        }
    }
}

pub(crate) fn event_supported(
    channel: &ResolvedChannel,
    event: &EventRefPayload,
    activity: &Activity,
) -> bool {
    let (
        SourceKind::Discord { bot_id: source_bot },
        ReplyAddress::Discord {
            bot_id, channel_id, ..
        },
    ) = (&activity.source, &event.reply)
    else {
        return false;
    };
    event.lineage.is_none()
        && event.verifies_activity(activity)
        && source_bot == bot_id
        && activity.conversation.id == *channel_id
        && event.channel_id == channel.id
        && event.record_id == channel.pointer.record_id
        && discord_edit::authority(channel, event.consumer.as_ref(), bot_id).is_some()
}

fn check<'a>(
    message: &'a MessageRefPayload,
    channel: &ResolvedChannel,
) -> Result<&'a DiscordEffectWindow, Problem> {
    let denied = || Problem::new(ProblemCode::Forbidden, 403, "message_effect_revoked");
    let destination = message.discord_message.as_ref().ok_or_else(denied)?;
    let window = destination.effects.as_ref().ok_or_else(denied)?;
    let now = super::now();
    if message.validate().is_err()
        || window.issued_at > now
        || message.channel_id != channel.id
        || token(channel, &destination.bot_id).is_none()
        || discord_edit::authority(channel, message.consumer.as_ref(), &destination.bot_id)
            != Some(destination.authority_sha256)
    {
        return Err(denied());
    }
    if window.expires_at <= now {
        return Err(Problem::new(
            ProblemCode::EventRefExpired,
            410,
            "message_effect_expired",
        ));
    }
    Ok(window)
}

pub(super) fn capabilities(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    client: bool,
) -> Result<MessageEffectsCapabilities, Problem> {
    if message
        .discord_message
        .as_ref()
        .and_then(|destination| destination.effects.as_ref())
        .is_none()
    {
        return Ok(MessageEffectsCapabilities::new(false, None));
    }
    let window = check(message, channel)?;
    Ok(MessageEffectsCapabilities::new(
        client,
        Some(window.expires_at),
    ))
}

impl ToolsState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn discord_message_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        let Some(client) = self.discord_effects.as_ref().filter(|_| {
            message
                .discord_message
                .as_ref()
                .and_then(|destination| destination.effects.as_ref())
                .is_some()
        }) else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        let window = check(&message, &channel)?;
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match store
            .claim_scoped(
                "discord_message_effect_v1",
                "cmaeg.discord-effect.v1",
                receipts::hash(
                    &serde_json::json!({"channel":message.channel_id,"consumer":message.consumer,
                "authority":message.discord_message,"message":message.platform_message_id}),
                ),
                request_id,
                receipts::hash(&effect),
                &channel.definition.owner.subject,
                window.expires_at,
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
        // Serialize different idempotency keys too: a reaction replacement is a
        // read/remove/add sequence, and overlapping keys must not leave two reactions.
        let serial = super::serial::Store::new(self.resolver.record_client(), self.keys.clone());
        let destination = message
            .discord_message
            .as_ref()
            .expect("checked native authority");
        let held = serial
            .acquire(
                receipts::hash(&serde_json::json!({"bot":destination.bot_id,"native_channel":destination.channel_id,
                "message":message.platform_message_id})),
                &channel.definition.owner.subject,
                window.expires_at,
            )
            .await?;
        let Some(held) = held else {
            let mut receipt = receipts::uncertain("effect_receipt_unconfirmed");
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_message_effect_busy".into(),
            };
            store.finish(claim, &receipt).await?;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        };
        // A revoked source, consumer or key cannot act after an earlier cached read.
        let (channel, message) = self.current_message(caller, reference).await?;
        check(&message, &channel)?;
        let destination = message
            .discord_message
            .as_ref()
            .expect("checked native authority");
        let credential = token(&channel, &destination.bot_id).expect("checked native credential");
        let sent = match effect {
            Effect::Delete => {
                client
                    .api
                    .delete_message(
                        credential,
                        &destination.bot_id,
                        &destination.channel_id,
                        &message.platform_message_id,
                    )
                    .await
            }
            Effect::Reaction { request } => {
                client
                    .api
                    .set_message_reaction(
                        credential,
                        &destination.bot_id,
                        &destination.channel_id,
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
            Err(RestError::RateLimited(retry_after)) => {
                DeliveryOutcome::RateLimited { retry_after }
            }
            Err(RestError::Unknown) => DeliveryOutcome::Unknown {
                reason: "native_delivery_unconfirmed".into(),
            },
            Err(RestError::Unavailable) => DeliveryOutcome::Rejected {
                reason: "native_dispatch_unavailable".into(),
            },
            Err(RestError::Rejected | RestError::NotFound) => DeliveryOutcome::Rejected {
                reason: "native_request_rejected".into(),
            },
        };
        let mut receipt = receipts::uncertain("effect_receipt_unconfirmed");
        receipt.delivery = delivery;
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unknown {
                    reason: "effect_receipt_unconfirmed".into(),
                },
            });
        }
        // Keep uncertain writes fenced through reference expiry. A new key or a
        // restarted replica cannot overlap a possibly late platform mutation.
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. })
            && serial.release(held).await.is_err()
        {
            tracing::warn!("native message effect fence remains held after a completed receipt");
        }
        Ok(MessageEffectReceipt {
            delivery: receipt.delivery,
        })
    }
}
