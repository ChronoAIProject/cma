use super::*;
use crate::sandbox::binding::{BindingAccessService, BindingConversationAuthority, BindingError};
use crate::{Authorization, Capability};
use std::sync::Arc;

impl ResourceAccessService {
    pub fn with_bindings(mut self, bindings: Arc<BindingAccessService>) -> Self {
        self.bindings = Some(bindings);
        self
    }

    pub(super) async fn authorize_binding(
        &self,
        context: &CallerContext,
        target: &ResourceTarget,
        capability: ResourceCapability,
    ) -> Result<AuthorizedResource, ResourceAccessError> {
        if context.credential_source != CredentialSource::Binding
            || context.credential.issuer != "binding:local"
            || context.caller.authorization
                != Authorization::new([Capability::UseBindingConversations])
            || !matches!(&context.actor, ResourceActor::Api {subject, delegation_id: None} if subject == &context.caller.subject)
        {
            return Err(AccessError::NotFound.into());
        }
        let ResourceTarget::Agent { agent_id } = target else {
            return Err(AccessError::NotFound.into());
        };
        let access = self.binding_access()?;
        let principal = access
            .authenticate(&context.credential.credential_id)
            .await
            .map_err(binding_error)?
            .ok_or(AccessError::NotFound)?;
        if principal.caller() != context.caller {
            return Err(AccessError::NotFound.into());
        }
        let proof = access
            .resolve(&principal, agent_id.as_str())
            .await
            .map_err(binding_error)?;
        let snapshot = self.store.resource_access_snapshot(target).await?;
        let lineage = &snapshot.lineage;
        let (Some(agent), Some(workspace), Some(worktree)) =
            (&lineage.agent, &lineage.workspace, &lineage.worktree)
        else {
            return Err(ResourceAccessStoreError::Corrupt.into());
        };
        if snapshot.scope(target)?.owner != proof.binding.resource_owner()
            || lineage.sandbox.id != proof.binding.sandbox_id
            || workspace.id != proof.binding.workspace_id
            || worktree.id != proof.conversation.worktree_id
            || agent.id != proof.conversation.agent_id
            || agent.owner != proof.binding.owner
        {
            return Err(AccessError::NotFound.into());
        }
        access.validate(&proof).await.map_err(binding_error)?;
        let decision = AccessDecision::binding(&proof, false);
        decision.require(capability)?;
        Ok(AuthorizedResource {
            snapshot,
            decision,
            binding: Some(proof),
        })
    }

    pub(crate) async fn validate_binding_authority(
        &self,
        authorized: &AuthorizedResource,
    ) -> Result<(), ResourceAccessError> {
        if let Some(proof) = &authorized.binding {
            self.binding_access()?
                .validate(proof)
                .await
                .map_err(binding_error)?;
        }
        Ok(())
    }

    fn binding_access(&self) -> Result<&BindingAccessService, ResourceAccessError> {
        self.bindings
            .as_deref()
            .ok_or_else(|| ResourceAccessStoreError::Unavailable.into())
    }
}

impl AuthorizedResource {
    pub fn binding_authority(&self) -> Option<&BindingConversationAuthority> {
        self.binding.as_ref()
    }
}

fn binding_error(error: BindingError) -> ResourceAccessError {
    match error {
        BindingError::NotFound | BindingError::Forbidden => AccessError::NotFound.into(),
        BindingError::Conflict
        | BindingError::SkillVersionUpdateUnsupported { .. }
        | BindingError::ReconfigurationUnsupported => ResourceAccessStoreError::Conflict.into(),
        BindingError::Corrupt(_) | BindingError::InvalidRequest(_) => {
            ResourceAccessStoreError::Corrupt.into()
        }
        BindingError::Unavailable(_) | BindingError::ResourcesUnavailable => {
            ResourceAccessStoreError::Unavailable.into()
        }
    }
}
