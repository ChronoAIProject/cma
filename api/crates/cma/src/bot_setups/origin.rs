//! Resolve callback destinations exclusively from admitted provider events.
use super::*;
use crate::auth::actor_links::ActorLinkOrigin;
use layer2_core::sandbox::{
    agent_io::{external_event_key, ExternalEventContext},
    binding::{BindingConversation, BindingConversationAuthority, ProviderBinding},
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct ReplyTarget {
    reply_target_ref: String,
    expires_at: u64,
}

impl SetupAuthority {
    pub(super) async fn origin_for_event(
        &self,
        event: &SetupEvent,
        reference: &str,
        credential: &str,
    ) -> Result<ActorLinkOrigin, BotError> {
        let (binding, conversation, source_event) = self
            .admitted_event(&event.actor, &event.event_id, reference, credential)
            .await?;
        let target: ReplyTarget = serde_json::from_value(
            self.json(
                self.http
                    .post(format!(
                        "{}/v1/events/{reference}/reply-target",
                        self.gateway
                    ))
                    .bearer_auth(credential)
                    .json(&serde_json::json!({})),
            )
            .await?,
        )
        .map_err(|_| BotError::Unavailable)?;
        // The binding may be rotated/reconfigured while the gateway is read.
        let (current, route, _) = self
            .admitted_event(&event.actor, &event.event_id, reference, credential)
            .await?;
        if current != binding || route != conversation {
            return Err(BotError::Conflict);
        }
        Ok(ActorLinkOrigin {
            binding_generation: binding.generation,
            provider_conversation_id: conversation.conversation_id,
            agent_id: conversation.agent_id,
            worktree_id: conversation.worktree_id,
            source_event,
            reply_target_ref: target.reply_target_ref,
            reply_expires_at: target.expires_at,
        })
    }

    pub(super) async fn admitted_event(
        &self,
        actor: &ExternalActor,
        event: &str,
        reference: &str,
        credential: &str,
    ) -> Result<(ProviderBinding, BindingConversation, ExternalEventContext), BotError> {
        let store = &self.store;
        let hash = format!("{:x}", Sha256::digest(credential.as_bytes()));
        let binding = store
            .find_by_key_hash(&hash)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        if binding.binding_id != actor.binding_id || binding.binding_id != self.binding {
            return Err(BotError::Forbidden);
        }
        let key = external_event_key(
            &format!("binding:{}", binding.binding_id),
            &actor.conversation_id,
            event,
        );
        let record = store
            .get_external_event_command(&key)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        record.validate().map_err(|_| BotError::Unavailable)?;
        let envelope = &record.receipt.envelope;
        let source = envelope
            .command
            .external_event()
            .ok_or(BotError::Forbidden)?;
        if source.reference != reference
            || source.source_id != actor.channel_id
            || source.conversation_id != actor.conversation_id
            || source.event_id != event
            || source.payload["activity"]["actor"]["id"].as_str() != Some(&actor.actor_id)
            || source.payload["activity"]["actor"]["kind"].as_str() != Some("human")
            || source.payload["activity"]["source"]["platform"].as_str() != Some(&actor.platform)
        {
            return Err(BotError::Forbidden);
        }
        let conversation = store
            .binding_conversation_for_agent(
                &binding.binding_id,
                binding.generation,
                &envelope.agent_id,
            )
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        BindingConversationAuthority {
            binding: binding.clone(),
            conversation: conversation.clone(),
        }
        .require_command(envelope)
        .map_err(|_| BotError::Forbidden)?;
        Ok((binding, conversation, source.clone()))
    }
}
