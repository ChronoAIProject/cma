//! Fresh native custody and local authority at every external mutation boundary.
use super::{
    lark_ledger,
    native_lark::{outcome, rejected, unknown, Action, NativeLarkEffectsClient},
    native_lark_authority as authority, ToolsState,
};
use crate::{auth::AgentCaller, delivery::intents::DeliveryOutcome};
use cmaeg_core::{
    channel::Secret,
    ids::MessageRef,
    seal::{MessageRefPayload, ReplyAddress},
};

impl ToolsState {
    async fn native_lark_before_write(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        client: &NativeLarkEffectsClient,
        editing: bool,
    ) -> Result<(MessageRefPayload, Secret), DeliveryOutcome> {
        let denied = |_| rejected("native_lark_message_authority_unavailable");
        let (channel, message) = self
            .current_message(caller, reference)
            .await
            .map_err(denied)?;
        authority::check(&message, &channel, caller).map_err(denied)?;
        authority::retained(self, &message).await.map_err(denied)?;
        let a = message
            .native_lark_effect
            .as_ref()
            .expect("checked authority");
        let reply = ReplyAddress::NativeLark(a.reply.clone());
        let credentials = crate::native_lark::credentials(&channel.definition, &reply)
            .ok_or_else(|| rejected("native_lark_credentials_revoked"))?;
        let token = client
            .client
            .verified_token(&a.reply.identity, credentials)
            .await
            .map_err(|_| rejected("native_lark_bot_custody_unverified"))?;
        client
            .client
            .effect_message(&token, a, editing)
            .await
            .map_err(|_| rejected("native_lark_message_unverified"))?;
        // Token and metadata reads may race source/consumer/provider changes.
        authority::retained(self, &message).await.map_err(denied)?;
        let (fresh, current) = self
            .current_message(caller, reference)
            .await
            .map_err(denied)?;
        authority::check(&current, &fresh, caller).map_err(denied)?;
        Ok((current, token))
    }
    pub(super) async fn native_lark_execute(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        client: &NativeLarkEffectsClient,
        action: &Action,
        serial: &lark_ledger::Store,
        held: &mut lark_ledger::Lock,
    ) -> DeliveryOutcome {
        let (message, token) = match self
            .native_lark_before_write(
                caller,
                reference,
                client,
                matches!(action, Action::Edit { .. }),
            )
            .await
        {
            Ok(result) => result,
            Err(error) => return error,
        };
        let a = message
            .native_lark_effect
            .as_ref()
            .expect("checked authority");
        let confirmed = || DeliveryOutcome::Confirmed {
            platform_message_id: Some(message.platform_message_id.clone()),
        };
        match action {
            Action::Delete => {
                if let Err(error) = client.client.effect_delete(&token, a).await {
                    return outcome(error);
                }
                if serial.retain(held, None).await.is_err() {
                    return unknown().delivery;
                }
            }
            Action::Edit { text } => {
                if let Err(error) = client.client.effect_edit(&token, a, text).await {
                    return outcome(error);
                }
            }
            Action::Reaction { emoji } => {
                let removed = if let Some(previous) = held.reaction.clone() {
                    if let Err(error) = client.client.effect_clear(&token, a, &previous).await {
                        return outcome(error);
                    }
                    if serial.retain(held, None).await.is_err() {
                        return unknown().delivery;
                    }
                    true
                } else {
                    false
                };
                let Some(emoji) = emoji else {
                    return confirmed();
                };
                // Replacement is two separate writes. Repeat native and local checks;
                // any refusal after removal leaves an uncertain partial mutation fenced.
                let token = if removed {
                    match self
                        .native_lark_before_write(caller, reference, client, false)
                        .await
                    {
                        Ok((_, token)) => token,
                        Err(_) => return unknown().delivery,
                    }
                } else {
                    token
                };
                let reaction = match client.client.effect_react(&token, a, emoji).await {
                    Ok(reaction) => reaction,
                    Err(error) => {
                        return if removed {
                            unknown().delivery
                        } else {
                            outcome(error)
                        }
                    }
                };
                if serial.retain(held, Some(reaction)).await.is_err() {
                    return unknown().delivery;
                }
            }
        }
        confirmed()
    }
}
