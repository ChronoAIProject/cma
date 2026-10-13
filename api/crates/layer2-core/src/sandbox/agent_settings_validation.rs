use super::store::{AgentConversationMutation, SandboxStoreError};
use super::{Agent, AgentOperationDelivery, AgentOperationIntent, AgentStatus, QueuedMessageState};

pub(super) fn validate_settings_mutation(
    current: &Agent,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    let next = &mutation.agent;
    if current.conversation.settings == next.conversation.settings {
        return Ok(());
    }
    let Some(operation) = next.conversation.operation.as_ref() else {
        return Err(SandboxStoreError::Conflict);
    };
    let AgentOperationIntent::UpdateSettings { settings, previous } = &operation.intent else {
        return Err(SandboxStoreError::Conflict);
    };
    let admission =
        operation.state == AgentOperationDelivery::DispatchFenced
            && current.conversation.operation.as_ref().is_none_or(|prior| {
                !prior.blocks_dispatch() && prior.sequence < operation.sequence
            })
            && mutation.admission_guard.is_some()
            && current.status == AgentStatus::Ready
            && next.status == AgentStatus::Ready
            && current.work.step == super::AgentWorkStep::None
            && current.conversation.active_turn.is_none()
            && current
                .conversation
                .queued_messages
                .iter()
                .all(|message| message.state == QueuedMessageState::Delivered)
            && previous == &current.conversation.settings
            && next.conversation.settings.as_ref() == Some(settings);
    let rollback = operation.state == AgentOperationDelivery::PreWriteFailed
        && mutation.admission_guard.is_none()
        && current
            .conversation
            .operation
            .as_ref()
            .is_some_and(|prior| {
                prior.sequence == operation.sequence
                    && prior.intent == operation.intent
                    && prior.state == AgentOperationDelivery::DispatchFenced
            })
        && current.conversation.settings.as_ref() == Some(settings)
        && &next.conversation.settings == previous;
    if admission || rollback {
        Ok(())
    } else {
        Err(SandboxStoreError::Conflict)
    }
}

pub(super) struct SettingsInputGuard {
    pub store: std::sync::Arc<dyn super::AgentOperationStore>,
    pub target: super::AgentRuntimeTarget,
    pub sequence: u64,
}

#[async_trait::async_trait]
impl layer3_adapter::ConversationInputGuard for SettingsInputGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        let refused = || {
            layer3_adapter::AdapterError::Unavailable(
                "agent settings authority changed before dispatch".into(),
            )
        };
        let agent = self
            .store
            .get_agent(&self.target.agent_id)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let parent = self
            .store
            .get_sandbox(&agent.sandbox_id)
            .await
            .map_err(|_| refused())?
            .ok_or_else(refused)?;
        let valid = agent.status == AgentStatus::Ready
            && agent.requested_stop_reason.is_none()
            && agent.work.step == super::AgentWorkStep::None
            && agent.conversation.conversation_id.as_ref() == Some(&self.target.conversation_id)
            && agent.conversation.runtime_generation == Some(self.target.runtime_generation)
            && agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.sequence == self.sequence
                        && operation.state == AgentOperationDelivery::DispatchFenced
                        && matches!(
                            operation.intent,
                            AgentOperationIntent::UpdateSettings { .. }
                        )
                })
            && parent.owner == agent.owner
            && matches!(
                parent.status,
                super::SandboxStatus::Active | super::SandboxStatus::CredentialRotating
            )
            && parent.lifecycle.checkpoint_capture.is_none();
        if valid {
            Ok(())
        } else {
            Err(refused())
        }
    }
}
