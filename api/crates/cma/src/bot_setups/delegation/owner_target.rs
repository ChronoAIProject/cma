//! Resolve an existing Agent Delete from the original event-bound operation.
//! Preflight is read only: no link creation, reply-target minting or Agent claim.
use super::*;
use crate::auth::{actor_links::ActorGrantScope, persistence::SecretValue};
use layer2_core::{resource_access::CallerContext, AgentId};

pub(crate) struct OwnerDeleteTarget {
    pub agent_id: AgentId,
    pub context: CallerContext,
    pub credential: SecretValue,
}
impl SetupDependencies {
    pub(crate) async fn owner_delete_target(
        &self,
        credential: &str,
        body: &[u8],
    ) -> Result<Option<OwnerDeleteTarget>, BotError> {
        self.authority.owner_delete_target(credential, body).await
    }
}
impl SetupAuthority {
    pub(crate) async fn owner_delete_target(
        &self,
        credential: &str,
        bytes: &[u8],
    ) -> Result<Option<OwnerDeleteTarget>, BotError> {
        let body: OperationRequest = serde_json::from_slice(bytes)
            .map_err(|_| BotError::Invalid("Invalid operation request.".into()))?;
        if body.method != "DELETE" {
            return Ok(None);
        }
        let path = body.path.split('?').next().unwrap_or_default();
        let Some(id) = path.strip_prefix("/api/v1/agents/") else {
            return Ok(None);
        };
        policy::validate(&Method::DELETE, &body.path, body.idempotency_key.as_deref())?;
        let id = AgentId(id.into());
        let request = self
            .auth
            .binding_request(credential.into())
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        let principal = request.binding.as_ref().ok_or(BotError::Forbidden)?;
        if principal.binding_id() != self.binding {
            return Err(BotError::Forbidden);
        }
        let (actor, event_id, reference) = if body.event_ref.starts_with("cma1.cont.") {
            let (actor, origin) = self
                .auth
                .authorized_actor_continuation(&body.event_ref)
                .await?;
            let external = ExternalActor {
                binding_id: actor.binding_id.clone(),
                channel_id: actor.channel_id.clone(),
                conversation_id: actor.conversation_id.clone(),
                actor_id: actor.actor_id.clone(),
                platform: actor.platform.clone(),
            };
            let (binding, conversation, source) = self
                .admitted_event(
                    &external,
                    &origin.source_event.event_id,
                    &origin.source_event.reference,
                    credential,
                )
                .await?;
            if binding.generation != origin.binding_generation
                || conversation.agent_id != origin.agent_id
                || conversation.worktree_id != origin.worktree_id
                || conversation.conversation_id != origin.provider_conversation_id
                || source != origin.source_event
            {
                return Err(BotError::Forbidden);
            }
            (
                actor,
                origin.source_event.event_id,
                origin.source_event.reference,
            )
        } else {
            let event = super::super::events::EventVerifier {
                http: &self.http,
                gateway: &self.gateway,
                binding: &self.binding,
                contexts: self.contexts.as_ref(),
            }
            .verify(&self.binding, credential, &body.event_ref, None)
            .await?;
            (
                super::super::authorization::actor(&event.actor),
                event.event_id,
                event.reference,
            )
        };
        let external = ExternalActor {
            binding_id: actor.binding_id.clone(),
            channel_id: actor.channel_id.clone(),
            conversation_id: actor.conversation_id.clone(),
            actor_id: actor.actor_id.clone(),
            platform: actor.platform.clone(),
        };
        self.admitted_event(&external, &event_id, &reference, credential)
            .await?;
        let Some(grant) = self.auth.existing_actor_grant(&actor).await? else {
            return Ok(None);
        };
        if grant.scope != ActorGrantScope::OrdinaryCma {
            return Err(BotError::Forbidden);
        }
        let owner = body.owner.unwrap_or(ResourceOwner::Person {
            subject: grant.subject.clone(),
        });
        policy::validate_owner(&Method::DELETE, &body.path, &owner)?;
        let (context, token) = self
            .auth
            .context_for_grant(&grant.grant_id, &actor, &owner)
            .await?;
        self.admitted_event(&external, &event_id, &reference, credential)
            .await?;
        Ok(Some(OwnerDeleteTarget {
            agent_id: id,
            context,
            credential: token,
        }))
    }
}
