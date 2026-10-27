//! Native card effects retain card custody at every boundary; derived refs stay internal.
use super::{
    lark_ledger,
    native_lark::{self, Action},
    native_lark_authority, receipts, Effect, MessageEffectReceipt, MessageEffectsCapabilities,
    ToolsState,
};
use crate::{
    auth::AgentCaller, channels::resolver::ResolvedChannel, delivery::intents::DeliveryOutcome,
    interactions::InteractionStore,
};
use cmaeg_core::{
    ids::MessageRef,
    problem::{Problem, ProblemCode},
    seal::{MessageRefPayload, ReplyAddress},
};
impl ToolsState {
    fn native_cards(&self) -> InteractionStore {
        InteractionStore::new(
            self.resolver.record_client(),
            self.keys.clone(),
            self.providers.clone(),
        )
    }
    pub(super) async fn native_lark_card_capabilities(
        &self,
        channel: &ResolvedChannel,
        message: &MessageRefPayload,
    ) -> Result<MessageEffectsCapabilities, Problem> {
        let (card, _) = self
            .native_cards()
            .native_lark_reference(channel, message)
            .await
            .map_err(|_| denied())?;
        let enabled = !card.superseded
            && crate::interactions::native_lark::mutation_ready(&card)
            && self.native_lark_effects.is_some();
        Ok(MessageEffectsCapabilities {
            delete: enabled,
            reaction: enabled,
            max_reactions: u8::from(enabled),
            scope: "confirmed_native_card".into(),
            expires_at: Some(card.expires_at),
        })
    }
    pub(super) async fn native_lark_card_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        key: &str,
        effect: Effect,
    ) -> Result<MessageEffectReceipt, Problem> {
        let action = native_lark::action(effect)?;
        let (channel, message) = self.current_message(caller, reference).await?;
        let cards = self.native_cards();
        let (card, _) = cards
            .native_lark_reference(&channel, &message)
            .await
            .map_err(|_| denied())?;
        let destination = card
            .native_lark
            .as_ref()
            .and_then(|a| a.destination.as_ref())
            .ok_or_else(denied)?;
        let Some(client) = &self.native_lark_effects else {
            return Ok(MessageEffectReceipt {
                delivery: DeliveryOutcome::Unsupported,
            });
        };
        let _capacity = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let serial_key = lark_ledger::native_destination(&destination.reply);
        let scope = receipts::hash(&(
            message.channel_id.clone(),
            message.consumer.clone(),
            &serial_key,
            &card.id,
        ));
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        if let Some(receipt) = store
            .lookup_scoped(
                "native_lark_card_effect_v1",
                "cmaeg.native-lark-card-effect.v1",
                scope.clone(),
                key,
                receipts::hash(&action),
                &channel.definition.owner.subject,
                destination.retained_until,
            )
            .await?
        {
            return Ok(MessageEffectReceipt {
                delivery: receipt.delivery,
            });
        }
        if card.superseded || !crate::interactions::native_lark::mutation_ready(&card) {
            return Err(denied());
        }
        let claim = match store
            .claim_scoped(
                "native_lark_card_effect_v1",
                "cmaeg.native-lark-card-effect.v1",
                scope,
                key,
                receipts::hash(&action),
                &channel.definition.owner.subject,
                destination.retained_until,
            )
            .await?
        {
            receipts::Admission::Replay(r) => {
                return Ok(MessageEffectReceipt {
                    delivery: r.delivery,
                })
            }
            receipts::Admission::Claimed(c) => c,
        };
        let serial = lark_ledger::Store::new(self.resolver.record_client(), self.keys.clone());
        let Some(mut held) = serial
            .acquire(
                serial_key,
                &channel.definition.owner.subject,
                destination.retained_until,
            )
            .await?
        else {
            let r = native_lark::unknown();
            store.finish(claim, &r).await?;
            return Ok(MessageEffectReceipt {
                delivery: r.delivery,
            });
        };
        let mut receipt = native_lark::unknown();
        receipt.delivery = self
            .native_lark_card_execute(caller, reference, &action, &serial, &mut held)
            .await;
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(MessageEffectReceipt {
                delivery: native_lark::unknown().delivery,
            });
        }
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        Ok(MessageEffectReceipt {
            delivery: receipt.delivery,
        })
    }
    async fn native_lark_card_before_write(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        deleting: bool,
    ) -> Result<
        (
            cmaeg_core::seal::native_lark_effect::NativeLarkEffectAuthority,
            cmaeg_core::channel::Secret,
        ),
        DeliveryOutcome,
    > {
        let refusal = || native_lark::rejected("native_lark_card_authority_unavailable");
        let (channel, message) = self
            .current_message(caller, reference)
            .await
            .map_err(|_| refusal())?;
        let cards = self.native_cards();
        let (card, _) = cards
            .native_lark_reference(&channel, &message)
            .await
            .map_err(|_| refusal())?;
        if card.superseded != deleting || !crate::interactions::native_lark::mutation_ready(&card) {
            return Err(refusal());
        }
        let destination = card
            .native_lark
            .as_ref()
            .and_then(|a| a.destination.as_ref())
            .ok_or_else(refusal)?;
        let derived = native_lark_authority::confirmed(&channel, destination, false, &self.keys)
            .ok_or_else(refusal)?;
        let effect = derived.open(&self.keys).map_err(|_| refusal())?.payload;
        native_lark_authority::check(&effect, &channel, caller).map_err(|_| refusal())?;
        let authority = *effect.native_lark_effect.ok_or_else(refusal)?;
        let reply = ReplyAddress::NativeLark(destination.reply.clone());
        let credentials =
            crate::native_lark::credentials(&channel.definition, &reply).ok_or_else(refusal)?;
        let client = &self
            .native_lark_effects
            .as_ref()
            .ok_or_else(refusal)?
            .client;
        let token = client
            .verified_token(&destination.reply.identity, credentials)
            .await
            .map_err(|_| refusal())?;
        client
            .card_message(&token, &destination.reply)
            .await
            .map_err(|_| refusal())?;
        let (fresh, current) = self
            .current_message(caller, reference)
            .await
            .map_err(|_| refusal())?;
        let (fresh_card, _) = cards
            .native_lark_reference(&fresh, &current)
            .await
            .map_err(|_| refusal())?;
        if fresh_card.superseded != deleting
            || !crate::interactions::native_lark::mutation_ready(&fresh_card)
        {
            return Err(refusal());
        }
        Ok((authority, token))
    }
    async fn native_lark_card_execute(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        action: &Action,
        serial: &lark_ledger::Store,
        held: &mut lark_ledger::Lock,
    ) -> DeliveryOutcome {
        let deleting = matches!(action, Action::Delete);
        if deleting {
            let Ok((channel, message)) = self.current_message(caller, reference).await else {
                return native_lark::rejected("native_lark_card_authority_unavailable");
            };
            if self
                .native_cards()
                .fence_native_lark(&channel, &message)
                .await
                .is_err()
            {
                return native_lark::rejected("native_lark_card_fence_unavailable");
            }
        }
        let (authority, token) = match self
            .native_lark_card_before_write(caller, reference, deleting)
            .await
        {
            Ok(v) => v,
            Err(e) => return e,
        };
        let client = &self
            .native_lark_effects
            .as_ref()
            .expect("checked client")
            .client;
        let confirmed = || DeliveryOutcome::Confirmed {
            platform_message_id: Some(authority.reply.message_id.clone()),
        };
        match action {
            Action::Delete => {
                if let Err(e) = client.effect_delete(&token, &authority).await {
                    return native_lark::outcome(e);
                }
                if serial.retain(held, None).await.is_err() {
                    return native_lark::unknown().delivery;
                }
            }
            Action::Reaction { emoji } => {
                let removed = if let Some(previous) = held.reaction.clone() {
                    if let Err(e) = client.effect_clear(&token, &authority, &previous).await {
                        return native_lark::outcome(e);
                    }
                    if serial.retain(held, None).await.is_err() {
                        return native_lark::unknown().delivery;
                    }
                    true
                } else {
                    false
                };
                let Some(emoji) = emoji else {
                    return confirmed();
                };
                let token = if removed {
                    match self
                        .native_lark_card_before_write(caller, reference, false)
                        .await
                    {
                        Ok((_, token)) => token,
                        Err(_) => return native_lark::unknown().delivery,
                    }
                } else {
                    token
                };
                let reaction = match client.effect_react(&token, &authority, emoji).await {
                    Ok(r) => r,
                    Err(e) => {
                        return if removed {
                            native_lark::unknown().delivery
                        } else {
                            native_lark::outcome(e)
                        }
                    }
                };
                if serial.retain(held, Some(reaction)).await.is_err() {
                    return native_lark::unknown().delivery;
                }
            }
            Action::Edit { .. } => return DeliveryOutcome::Unsupported,
        }
        confirmed()
    }
}
fn denied() -> Problem {
    Problem::new(
        ProblemCode::Forbidden,
        403,
        "native_lark_card_authority_unavailable",
    )
}
