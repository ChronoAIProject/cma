//! Native card effects reuse saved authority; no new public token or effect window.
use super::{receipts, Effect, MessageEffectReceipt, MessageEffectsCapabilities, ToolsState};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::intents::DeliveryOutcome,
    discord::{rest::token, RestError},
    interactions::{Error, InteractionStore},
};
use cmaeg_core::{
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::MessageRefPayload,
};
use std::sync::Arc;
const KIND: &str = "discord_card_effect_v1";
const NAMESPACE: &str = "cmaeg.discord-card-effect.v1";

impl ToolsState {
    fn card_store(&self) -> InteractionStore {
        InteractionStore::new(
            self.resolver.record_client(),
            self.keys.clone(),
            self.providers.clone(),
        )
    }
    pub(super) async fn discord_card_capabilities(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectsCapabilities, Problem> {
        let Some(client) = &self.discord_effects else {
            return Ok(MessageEffectsCapabilities::new(false, None));
        };
        let store = self.card_store();
        let candidate = store
            .discord_card_candidate(&channel, &message)
            .await
            .map_err(problem)?;
        let expiry = candidate.card.expires_at;
        if candidate.card.superseded
            || (candidate.card.discord_effect.is_none() && !client.card_effects_enabled)
        {
            return Ok(MessageEffectsCapabilities::new(false, Some(expiry)));
        }
        if candidate.card.discord_effect.is_none() {
            let serial = store.mutation_store();
            let held = serial
                .acquire(
                    candidate.mutation_destination(),
                    &channel.definition.owner.subject,
                    expiry,
                )
                .await?;
            let Some(held) = held else {
                return Ok(MessageEffectsCapabilities::new(false, Some(expiry)));
            };
            // Capability adoption itself is retained, so turning off admission
            // later does not revoke an already advertised card capability.
            let (channel, message) = self.current_message(caller, reference).await?;
            let candidate = store
                .discord_card_candidate(&channel, &message)
                .await
                .map_err(problem)?;
            store
                .begin_discord_effect(candidate, None, client.card_effects_enabled)
                .await
                .map_err(problem)?;
            serial.release(held).await?;
        }
        Ok(MessageEffectsCapabilities::new(true, Some(expiry)))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn discord_card_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        let Some(client) = &self.discord_effects else {
            return Ok(result(DeliveryOutcome::Unsupported));
        };
        let cards = self.card_store();
        let candidate = cards
            .discord_card_candidate(&channel, &message)
            .await
            .map_err(problem)?;
        let expiry = candidate.card.expires_at;
        let scope = candidate.effect_scope();
        let request_hash = receipts::hash(&effect);
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        // Current key/source/provider custody is checked above, even on replay.
        // Saved results remain replayable after deletion or a successor revision.
        if let Some(receipt) = store
            .lookup_scoped(
                KIND,
                NAMESPACE,
                scope.clone(),
                request_id,
                request_hash.clone(),
                &channel.definition.owner.subject,
                expiry,
            )
            .await?
        {
            return Ok(result(receipt.delivery));
        }
        if candidate.card.superseded
            || (candidate.card.discord_effect.is_none() && !client.card_effects_enabled)
        {
            return Ok(result(DeliveryOutcome::Unsupported));
        }
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let claim = match store
            .claim_scoped(
                KIND,
                NAMESPACE,
                scope.clone(),
                request_id,
                request_hash,
                &channel.definition.owner.subject,
                expiry,
            )
            .await?
        {
            receipts::Admission::Replay(receipt) => return Ok(result(receipt.delivery)),
            receipts::Admission::Claimed(claim) => claim,
        };
        let serial = cards.mutation_store();
        let held = serial
            .acquire(
                candidate.mutation_destination(),
                &channel.definition.owner.subject,
                expiry,
            )
            .await?;
        let Some(held) = held else {
            let mut receipt = receipts::uncertain("effect_receipt_unconfirmed");
            receipt.delivery = DeliveryOutcome::Rejected {
                reason: "native_message_effect_busy".into(),
            };
            if store.finish(claim, &receipt).await.is_err() {
                return Ok(unknown());
            }
            return Ok(result(receipt.delivery));
        };
        let deleting = matches!(effect, Effect::Delete);
        let claim_id = claim.id().to_owned();
        // Destination lock always precedes card CAS. Reload after any click CAS
        // winner; its original consumed event remains immutable during deletion.
        let mut admitted = None;
        for _ in 0..2 {
            let (channel, message) = self.current_message(caller, reference).await?;
            let candidate = cards
                .discord_card_candidate(&channel, &message)
                .await
                .map_err(problem)?;
            if candidate.effect_scope() != scope {
                return Err(problem(Error::Denied));
            }
            match cards
                .begin_discord_effect(
                    candidate,
                    deleting.then_some(claim_id.as_str()),
                    client.card_effects_enabled,
                )
                .await
            {
                Ok(()) => {
                    admitted = Some((channel, message));
                    break;
                }
                Err(Error::Denied) => continue,
                Err(_) => return Ok(unknown()),
            }
        }
        let Some((_, message)) = admitted else {
            return Ok(unknown());
        };
        // Re-resolve after durable admission too: revocation during a record CAS
        // cannot authorize a write. Failure leaves its claim and lock fenced.
        let (channel, _) = self.current_message(caller, reference).await?;
        let candidate = cards
            .discord_card_candidate(&channel, &message)
            .await
            .map_err(problem)?;
        let identity = candidate.card.discord.as_ref().expect("checked card");
        let credential = token(&channel, &identity.bot_id).ok_or_else(|| problem(Error::Denied))?;
        let verified = async {
            client.api.verify_bot(credential, &identity.bot_id).await?;
            let thread = client
                .api
                .verify_component_channel(
                    credential,
                    &candidate.conversation.id,
                    &candidate.conversation,
                )
                .await?;
            if thread != identity.thread {
                return Err(RestError::Rejected);
            }
            Ok(())
        }
        .await;
        let sent = match verified {
            Err(error) => Err(error),
            Ok(()) => {
                // Channel/provider/key checks follow the platform metadata reads,
                // immediately before the single bounded native operation.
                let (fresh, _) = self.current_message(caller, reference).await?;
                let latest = cards
                    .discord_card_candidate(&fresh, &message)
                    .await
                    .map_err(problem)?;
                if latest.effect_scope() != scope {
                    return Err(problem(Error::Denied));
                }
                let credential =
                    token(&fresh, &identity.bot_id).ok_or_else(|| problem(Error::Denied))?;
                match effect {
                    Effect::Delete => {
                        client
                            .api
                            .delete_message(
                                credential,
                                &identity.bot_id,
                                &candidate.conversation.id,
                                &message.platform_message_id,
                            )
                            .await
                    }
                    Effect::Reaction { request } => {
                        client
                            .api
                            .set_message_reaction(
                                credential,
                                &identity.bot_id,
                                &candidate.conversation.id,
                                &message.platform_message_id,
                                &request,
                            )
                            .await
                    }
                }
            }
        };
        let mut receipt = receipts::uncertain("effect_receipt_unconfirmed");
        receipt.delivery = match sent {
            Ok(()) => DeliveryOutcome::Confirmed {
                platform_message_id: Some(message.platform_message_id.clone()),
            },
            Err(error) => delivery(error),
        };
        if deleting
            && cards
                .finish_discord_delete(&candidate.card.id, &claim_id, &receipt.delivery)
                .await
                .is_err()
        {
            return Ok(unknown());
        }
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(unknown());
        }
        // Unknown transport, partial reaction or lost final storage ack retains
        // this same message lock through the original card expiry.
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. })
            && serial.release(held).await.is_err()
        {
            tracing::warn!("native card mutation fence remains held after completed receipt");
        }
        Ok(result(receipt.delivery))
    }
}
fn result(delivery: DeliveryOutcome) -> MessageEffectReceipt {
    MessageEffectReceipt { delivery }
}
fn unknown() -> MessageEffectReceipt {
    result(DeliveryOutcome::Unknown {
        reason: "effect_receipt_unconfirmed".into(),
    })
}
fn problem(error: Error) -> Problem {
    match error {
        Error::Denied => Problem::new(ProblemCode::Forbidden, 403, "message_effect_revoked"),
        Error::Unavailable => Problem::new(
            ProblemCode::Internal,
            503,
            "message_effect_authority_unavailable",
        ),
    }
}

fn delivery(error: RestError) -> DeliveryOutcome {
    match error {
        RestError::RateLimited(retry_after) => DeliveryOutcome::RateLimited { retry_after },
        RestError::Unknown => DeliveryOutcome::Unknown {
            reason: "native_delivery_unconfirmed".into(),
        },
        RestError::Unavailable => DeliveryOutcome::Rejected {
            reason: "native_dispatch_unavailable".into(),
        },
        RestError::Rejected | RestError::NotFound => DeliveryOutcome::Rejected {
            reason: "native_request_rejected".into(),
        },
    }
}
