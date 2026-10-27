//! Exact-message fences span native edits, recalls and this app's retained reactions.
use super::{
    lark_ledger, native_lark_authority as authority, receipts, Effect, MessageEffectReceipt,
    MessageEffectsCapabilities, ToolsState,
};
use crate::{
    auth::AgentCaller,
    channels::resolver::ResolvedChannel,
    delivery::intents::{DeliveryOutcome, IntentReceipt},
    native_lark::{Client, Error},
};
use cmaeg_core::{
    ids::MessageRef,
    message::OutboundMessage,
    message_effect::TelegramReaction,
    problem::{Problem, ProblemCode},
    seal::{native_lark_effect::MAX_AGE, MessageRefPayload},
};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::Semaphore;

pub struct NativeLarkEffectsClient {
    pub controls_issuance_enabled: bool,
    pub forms_issuance_enabled: bool,
    pub confirmed_issuance_enabled: bool,
    pub received_issuance_enabled: bool,
    pub(crate) client: Client,
    pub(super) capacity: Semaphore,
}
impl NativeLarkEffectsClient {
    pub fn with_controls(mut self, controls: bool, forms: bool) -> Self {
        self.controls_issuance_enabled = controls;
        self.forms_issuance_enabled = controls && forms;
        self
    }

    pub fn new(client: Client, confirmed: bool, received: bool) -> Self {
        Self {
            client,
            controls_issuance_enabled: false,
            forms_issuance_enabled: false,
            confirmed_issuance_enabled: confirmed,
            received_issuance_enabled: received,
            capacity: Semaphore::new(32),
        }
    }
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum Action {
    Delete,
    Reaction { emoji: Option<String> },
    Edit { text: String },
}
pub(super) fn capabilities(
    message: &MessageRefPayload,
    channel: &ResolvedChannel,
    caller: AgentCaller,
    enabled: bool,
) -> Result<MessageEffectsCapabilities, Problem> {
    authority::check(message, channel, caller)?;
    let a = message
        .native_lark_effect
        .as_ref()
        .expect("checked authority");
    Ok(MessageEffectsCapabilities {
        delete: enabled && a.confirmed(),
        reaction: enabled,
        scope: if a.confirmed() {
            "confirmed_native_bot_message"
        } else {
            "received_native_user_message"
        }
        .into(),
        max_reactions: u8::from(enabled),
        expires_at: Some(a.expires_at),
    })
}
impl ToolsState {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn native_lark_message_effect(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        effect: Effect,
        channel: Arc<ResolvedChannel>,
        message: MessageRefPayload,
    ) -> Result<MessageEffectReceipt, Problem> {
        authority::check(&message, &channel, caller)?;
        let action = action(effect)?;
        let receipt = self
            .native_lark_mutation(caller, reference, request_id, action)
            .await?;
        Ok(MessageEffectReceipt {
            delivery: receipt.delivery,
        })
    }
    pub(crate) async fn native_lark_update(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        message: &OutboundMessage,
    ) -> Result<IntentReceipt, Problem> {
        use crate::delivery::render::Renderer;
        if !message.native_attachments.is_empty()
            || message.whatsapp_template.is_some()
            || message.reply_to.is_some()
            || message.thread_reply
            || crate::interactions::has_controls(message)
        {
            return Ok(unsupported());
        }
        let (parts, rendering) = crate::delivery::render::RelayText
            .render(message, &crate::tools::Capabilities::native_lark());
        if parts.len() != 1 || parts[0].text.is_empty() || parts[0].text.len() > 28 * 1024 {
            return Ok(unsupported());
        }
        // updateMessage has no caller idempotency key. Its durable destination fence
        // still prevents any new attempt after an ambiguous update, even after restart.
        let key = uuid::Uuid::new_v4().to_string();
        let mut receipt = self
            .native_lark_mutation(
                caller,
                reference,
                &key,
                Action::Edit {
                    text: parts[0].text.clone(),
                },
            )
            .await?;
        receipt.rendering = rendering;
        Ok(receipt)
    }
    pub(super) async fn native_lark_mutation(
        &self,
        caller: AgentCaller,
        reference: &MessageRef,
        request_id: &str,
        action: Action,
    ) -> Result<IntentReceipt, Problem> {
        let (channel, message) = self.current_message(caller, reference).await?;
        authority::check(&message, &channel, caller)?;
        authority::retained(self, &message).await?;
        let a = message
            .native_lark_effect
            .as_ref()
            .expect("checked authority");
        let Some(client) = &self.native_lark_effects else {
            return Ok(unsupported());
        };
        if matches!(action, Action::Delete) && !a.confirmed()
            || matches!(action, Action::Edit { .. }) && !a.editable()
        {
            return Ok(unsupported());
        }
        let _permit = client
            .capacity
            .try_acquire()
            .map_err(|_| Problem::new(ProblemCode::RateLimited, 429, "message_effect_capacity"))?;
        let destination = lark_ledger::native_destination(&a.reply);
        // Stable logical identity excludes ciphertext, issuance and credential revisions.
        let scope = receipts::hash(
            &serde_json::json!({"destination":destination,"channel":message.channel_id,
            "record":a.record_id,"consumer":message.consumer}),
        );
        let expires_at = a.observed_at.saturating_add(MAX_AGE);
        let store = receipts::Store::new(self.resolver.record_client(), self.keys.clone());
        let claim = match store
            .claim_scoped(
                "native_lark_message_effect_v1",
                "cmaeg.native-lark-effect.v1",
                scope,
                request_id,
                receipts::hash(&action),
                &channel.definition.owner.subject,
                expires_at,
            )
            .await?
        {
            receipts::Admission::Replay(receipt) => return Ok(receipt),
            receipts::Admission::Claimed(claim) => claim,
        };
        let serial = lark_ledger::Store::new(self.resolver.record_client(), self.keys.clone());
        let Some(mut held) = serial
            .acquire(destination, &channel.definition.owner.subject, expires_at)
            .await?
        else {
            let receipt = unknown();
            store.finish(claim, &receipt).await?;
            return Ok(receipt);
        };
        let outcome = self
            .native_lark_execute(caller, reference, client, &action, &serial, &mut held)
            .await;
        let mut receipt = unknown();
        receipt.delivery = outcome;
        if store.finish(claim, &receipt).await.is_err() {
            return Ok(unknown());
        }
        // Keep the fence permanently held on ambiguous writes or storage results.
        if !matches!(receipt.delivery, DeliveryOutcome::Unknown { .. }) {
            let _ = serial.release(held).await;
        }
        Ok(receipt)
    }
}
pub(super) fn unknown() -> IntentReceipt {
    receipts::uncertain("native_lark_message_mutation_unconfirmed")
}
fn unsupported() -> IntentReceipt {
    let mut r = unknown();
    r.delivery = DeliveryOutcome::Unsupported;
    r
}
pub(super) fn rejected(reason: &str) -> DeliveryOutcome {
    DeliveryOutcome::Rejected {
        reason: reason.into(),
    }
}
pub(super) fn outcome(error: Error) -> DeliveryOutcome {
    match error {
        Error::Invalid | Error::Rejected => rejected("native_lark_message_mutation_rejected"),
        Error::RateLimited => DeliveryOutcome::RateLimited { retry_after: 1 },
        Error::Unconfirmed => unknown().delivery,
    }
}

pub(super) fn action(effect: Effect) -> Result<Action, Problem> {
    match effect {
        Effect::Delete => Ok(Action::Delete),
        Effect::Reaction { request } => {
            if request.is_big || request.reaction.len() > 1 {
                return Err(super::invalid(
                    "Lark requires one emoji_type without is_big",
                ));
            }
            let emoji = match request.reaction.first() {
                None => None,
                Some(TelegramReaction::Emoji { emoji })
                    if !emoji.is_empty()
                        && emoji.len() <= 64
                        && emoji
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_') =>
                {
                    Some(emoji.clone())
                }
                _ => {
                    return Err(super::invalid(
                        "Lark requires a platform emoji_type such as THUMBSUP",
                    ))
                }
            };
            Ok(Action::Reaction { emoji })
        }
    }
}
