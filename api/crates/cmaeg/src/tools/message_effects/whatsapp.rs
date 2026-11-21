//! Set only this bot's reaction on one received, authenticated native message.
use super::{
    mutation, receipts, whatsapp_authority, Effect, MessageEffectReceipt,
    MessageEffectsCapabilities, ToolsState,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
};
use cmaeg_core::{
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::MessageRefPayload,
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Semaphore;
use utoipa::ToSchema;

pub struct WhatsappReactionsClient {
    pub issuance_enabled: bool,
    pub(super) client: crate::whatsapp::Client,
    capacity: Semaphore,
}
impl WhatsappReactionsClient {
    pub fn new(client: crate::whatsapp::Client, issuance_enabled: bool) -> Self {
        Self {
            client,
            issuance_enabled,
            capacity: Semaphore::new(32),
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ReceivedMessageContext {
    pub message_ref: MessageRef,
    pub effects: MessageEffectsCapabilities,
}

pub(super) fn capabilities(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
    enabled: bool,
) -> Result<MessageEffectsCapabilities, Problem> {
    whatsapp_authority::check(message, channel, caller)?;
    Ok(MessageEffectsCapabilities {
        media_edit: None,
        delete: false,
        reaction: enabled,
        scope: "received_native_user_message".into(),
        max_reactions: u8::from(enabled),
        expires_at: message.whatsapp_reaction.as_ref().map(|a| a.expires_at),
    })
}

impl ToolsState {
    /// Optional enrichment: retained-plan failure must not break ordinary context.
    pub(crate) async fn received_message_context(
        &self,
        caller: AgentCaller,
        event: &cmaeg_core::seal::EventRefPayload,
        activity: &cmaeg_core::activity::Activity,
        verified: bool,
    ) -> Option<ReceivedMessageContext> {
        if matches!(
            activity.source,
            cmaeg_core::activity::SourceKind::Telegram { .. }
                | cmaeg_core::activity::SourceKind::Discord { .. }
        ) {
            return self
                .native_received_message_context(caller, event, activity, verified)
                .await;
        }
        if matches!(
            activity.source,
            cmaeg_core::activity::SourceKind::NativeLark(_)
        ) {
            return self
                .native_lark_received_context(caller, event, activity, verified)
                .await;
        }
        if !verified
            || !self
                .resolver
                .record_client()
                .whatsapp_reaction_storage_available()
            || !self.whatsapp_reactions.as_ref()?.issuance_enabled
            || !matches!(
                activity.source,
                cmaeg_core::activity::SourceKind::Whatsapp { .. }
            )
        {
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
        let message_ref = whatsapp_authority::issue(&channel, event, activity, &self.keys)?;
        let message = message_ref.open(&self.keys).ok()?.payload;
        let effects = capabilities(&message, &channel, caller, true).ok()?;
        Some(ReceivedMessageContext {
            message_ref,
            effects,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn whatsapp_message_reaction(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        whatsapp_authority::check(&message, &channel, caller)?;
        let (Effect::Reaction { request }, Some(client)) = (&effect, &self.whatsapp_reactions)
        else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        if request.reaction.len() > 1
            || request.is_big
            || request.reaction.iter().any(|r| {
                matches!(
                    r,
                    cmaeg_core::message_effect::TelegramReaction::CustomEmoji { .. }
                )
            })
        {
            return Err(super::invalid(
                "WhatsApp requires at most one Unicode emoji without is_big",
            ));
        }
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let authority = message
            .whatsapp_reaction
            .as_ref()
            .expect("checked authority");
        // Logical identity survives new ciphertext and legitimate credential rotation.
        // A reference's shorter event expiry must not shorten durable target safety.
        let scope = receipts::hash(&serde_json::json!({
            "channel": message.channel_id, "event": message.event_id,
            "consumer": message.consumer, "app": authority.app_id,
            "waba": authority.waba_id, "phone": authority.phone_number_id,
            "recipient": authority.recipient_id, "message": message.platform_message_id,
        }));
        let retained_until = authority
            .received_at
            .saturating_add(cmaeg_core::seal::whatsapp_reaction::MAX_REACTION_AGE);
        let store =
            receipts::Store::whatsapp_reaction(self.resolver.record_client(), self.keys.clone());
        // Legacy HTTP storage can replay existing results but cannot admit a new
        // thirty-day claim or fence under its seven-day write contract.
        if !self
            .resolver
            .record_client()
            .whatsapp_reaction_storage_available()
        {
            let receipt = store
                .lookup_scoped(
                    "whatsapp_received_reaction_v1",
                    "cmaeg.whatsapp-received-reaction.v1",
                    scope,
                    request_id,
                    receipts::hash(&effect),
                    &channel.definition.owner.subject,
                    retained_until,
                )
                .await?
                .ok_or_else(|| {
                    Problem::new(
                        ProblemCode::Internal,
                        503,
                        "whatsapp_reaction_retention_unsupported",
                    )
                })?;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        }
        let claim = match store
            .claim_scoped(
                "whatsapp_received_reaction_v1",
                "cmaeg.whatsapp-received-reaction.v1",
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
        let serial =
            mutation::Store::whatsapp_reaction(self.resolver.record_client(), self.keys.clone());
        let held = serial
            .acquire(
                mutation::destination(
                    &authority.phone_number_id,
                    &authority.recipient_id,
                    &message.platform_message_id,
                ),
                &channel.definition.owner.subject,
                retained_until,
            )
            .await?;
        let Some(held) = held else {
            let receipt = receipts::uncertain("whatsapp_reaction_busy_or_uncertain");
            store.finish(claim, &receipt).await?;
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        };
        // Recheck current credentials, source, provider and caller after the CAS.
        // Refusal before a remote write releases the target, not its receipt.
        let current = async {
            let (channel, message) = self.current_message(caller, reference).await?;
            whatsapp_authority::check(&message, &channel, caller)?;
            Ok::<_, Problem>((channel, message))
        }
        .await;
        let (fresh, message) = match current {
            Ok(current) => current,
            Err(problem) => {
                let mut receipt = receipts::uncertain("whatsapp_reaction_revoked");
                receipt.delivery = DeliveryOutcome::Rejected {
                    reason: "whatsapp_reaction_revoked".into(),
                };
                store.finish(claim, &receipt).await?;
                let _ = serial.release(held).await;
                return Err(problem);
            }
        };
        let authority = message
            .whatsapp_reaction
            .as_ref()
            .expect("checked authority");
        let credentials = fresh
            .definition
            .secrets
            .whatsapp
            .as_ref()
            .expect("checked credentials");
        let sent = client
            .client
            .send_reaction(
                credentials,
                &authority.phone_number_id,
                &authority.recipient_id,
                &message.platform_message_id,
                request,
            )
            .await;
        let mut receipt = receipts::uncertain("whatsapp_reaction_unconfirmed");
        receipt.delivery = match sent {
            Ok(_) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(message.platform_message_id.clone()),
            },
            Err(crate::whatsapp::Error::RateLimited) => {
                DeliveryOutcome::RateLimited { retry_after: 1 }
            }
            Err(
                crate::whatsapp::Error::Rejected
                | crate::whatsapp::Error::Invalid
                | crate::whatsapp::Error::CustodyUnproved,
            ) => DeliveryOutcome::Rejected {
                reason: "whatsapp_reaction_rejected".into(),
            },
            Err(crate::whatsapp::Error::Unconfirmed) => DeliveryOutcome::Unknown {
                reason: "whatsapp_reaction_unconfirmed".into(),
            },
        };
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unknown {
                    reason: "whatsapp_reaction_receipt_unconfirmed".into(),
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
