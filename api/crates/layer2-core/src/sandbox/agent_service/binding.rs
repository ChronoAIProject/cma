use super::{support::runtime_error, *};
use crate::sandbox::binding::{BindingAdmissionFence, BindingConversation, ProviderBinding};
use crate::sandbox::{
    store::{AgentControlTarget, AgentDeleteRequest, AgentStopRequest},
    AgentStopReason,
};

impl AgentService {
    pub(crate) async fn create_for_binding(
        &self,
        binding: &ProviderBinding,
        conversation: &BindingConversation,
        revision: &AgentProfileRevision,
    ) -> Result<Agent, AgentServiceError> {
        self.runtime_capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentServiceError::CapabilityUnsupported { capability })?;
        let workspace = self
            .store
            .get_workspace(&binding.workspace_id)
            .await
            .map_err(store_error)?
            .filter(|value| value.owner == binding.owner && value.sandbox_id == binding.sandbox_id)
            .ok_or(AgentServiceError::WorkspaceNotFound)?;
        let sandbox = self
            .store
            .get_sandbox(&binding.sandbox_id)
            .await
            .map_err(store_error)?
            .filter(|value| value.owner == binding.owner)
            .ok_or(AgentServiceError::SandboxNotReady)?;
        let input = AgentAdmission {
            binding: Some(BindingAdmissionFence::conversation(binding, conversation)),
            created_by: Some(crate::resource_access::ResourceActor::Api {
                subject: binding.manager.clone(),
                delegation_id: None,
            }),
            owner: binding.owner.clone(),
            workspace_id: workspace.id,
            agent_id: conversation.agent_id.clone(),
            worktree_id: conversation.worktree_id.clone(),
            profile: AgentProfileSnapshot::from(revision),
            capabilities: self.runtime_capabilities.clone(),
            title: None,
            first_message: None,
            origin: AgentOrigin::Manual,
            creation_key: conversation.creation_key().map_err(store_error)?,
            event_at: now_rfc3339(),
        };
        input
            .binding
            .as_ref()
            .unwrap()
            .require_agent(binding, conversation, &input)
            .map_err(store_error)?;
        if let Some(existing) = self
            .store
            .get_agent(&conversation.agent_id)
            .await
            .map_err(store_error)?
        {
            if existing.owner != binding.owner
                || existing.sandbox_id != binding.sandbox_id
                || existing.workspace_id != binding.workspace_id
                || existing.worktree_id != conversation.worktree_id
                || existing.creation_key != input.creation_key
                || existing.profile != input.profile
            {
                return Err(AgentServiceError::CreationConflict);
            }
            return Ok(existing);
        }
        self.connections.check_sandbox(&sandbox).await?;
        let result = self.store.admit_agent(input).await.map_err(store_error)?;
        if let AgentAdmissionResult::Created(creation) = &result {
            self.record_profile_use(&creation.agent).await;
        }
        Ok(result.creation().agent.clone())
    }
}

impl AgentService {
    pub(crate) async fn retire_for_binding(
        &self,
        conversation: &BindingConversation,
    ) -> Result<bool, AgentServiceError> {
        if conversation.state != crate::sandbox::binding::BindingConversationState::Retiring {
            return Err(AgentServiceError::ControlConflict);
        }
        let Some(agent) = self
            .store
            .get_agent(&conversation.agent_id)
            .await
            .map_err(store_error)?
        else {
            return Ok(true);
        };
        if agent.owner != conversation.owner
            || agent.sandbox_id != conversation.sandbox_id
            || agent.workspace_id != conversation.workspace_id
            || agent.worktree_id != conversation.worktree_id
            || agent.creation_key != conversation.creation_key().map_err(store_error)?
        {
            return Err(AgentServiceError::CreationConflict);
        }
        if agent.status == AgentStatus::Deleted {
            return Ok(true);
        }
        if agent.status.is_live() {
            let scheduled = self
                .store
                .schedule_agent_stop(AgentStopRequest {
                    target: AgentControlTarget::from(&agent),
                    reason: agent
                        .requested_stop_reason
                        .unwrap_or(AgentStopReason::BindingRetired),
                    requested_at: now_rfc3339(),
                })
                .await
                .map_err(control_error)?
                .into_agent();
            self.runtimes
                .fence(&scheduled.id, scheduled.work.next_fence)
                .await
                .map_err(runtime_error)?;
        } else {
            self.store
                .schedule_agent_delete(AgentDeleteRequest {
                    target: AgentControlTarget::from(&agent),
                    requested_at: now_rfc3339(),
                })
                .await
                .map_err(control_error)?;
        }
        Ok(false)
    }
}
