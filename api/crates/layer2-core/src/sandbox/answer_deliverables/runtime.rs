use super::*;
use crate::sandbox::agent_access::AgentAccessError;
use crate::sandbox::agent_access_tool::AgentAccessToolAuthenticator;
use crate::{Authorization, Capability};

impl AnswerDeliverableService {
    pub async fn describe_for_agent(
        &self,
        auth: &dyn AgentAccessToolAuthenticator,
        capability: &str,
        resource: DeliverableResource,
    ) -> Result<DeliverableDescription, DeliverableError> {
        if matches!(resource, DeliverableResource::Document { .. }) {
            return Err(DeliverableError::Invalid);
        }
        let requester = auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?;
        let caller = Caller::new(
            requester.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        );
        let lineage = self.reads.lineage(&caller, &requester.agent_id).await?;
        if lineage.agent.created_at != requester.creation.agent_created_at
            || lineage.agent.creation_fingerprint != requester.creation.creation_fingerprint
            || lineage.agent.conversation.conversation_id.as_ref() != Some(&requester.conversation)
            || lineage.agent.workspace_id != requester.creation.workspace_id
            || lineage.agent.worktree_id != requester.creation.worktree_id
            || lineage.sandbox.id != requester.sandbox_id
            || lineage.sandbox.created_at != requester.creation.sandbox_created_at
            || lineage.sandbox.runtime.as_ref() != Some(&requester.runtime)
        {
            return Err(DeliverableError::NotFound);
        }
        let access = DeliverableAccess {
            caller: &caller,
            context: None,
            credential: None,
            document_token: None,
        };
        let result = self
            .describe(&access, &requester.agent_id, resource)
            .await?;
        if auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?
            != requester
        {
            return Err(DeliverableError::NotFound);
        }
        Ok(result)
    }
}

fn authority_error(error: AgentAccessError) -> DeliverableError {
    if matches!(error, AgentAccessError::Unavailable) {
        DeliverableError::Unavailable
    } else {
        DeliverableError::NotFound
    }
}
