use layer3_adapter::{AdapterError, ConversationInputGuard};

use crate::sandbox::{AgentOperationDelivery, AgentRuntimeTarget, SandboxStatus};

use super::CommandExecutionAuthorization;

pub(in crate::sandbox) struct CommandDispatchGuard {
    pub authority: CommandExecutionAuthorization,
    pub target: AgentRuntimeTarget,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
}

#[async_trait::async_trait]
impl ConversationInputGuard for CommandDispatchGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        let refused =
            || AdapterError::Rejected("command execution authority is no longer current".into());
        let snapshot = self.authority.current_snapshot().await.map_err(|error| {
            use crate::resource_access::{
                AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
            };
            if self.authority.continuation_envelope().is_some()
                && matches!(
                    error,
                    ResourceAccessError::Store(ResourceAccessStoreError::Unavailable)
                        | ResourceAccessError::Membership(MembershipError::Unavailable)
                        | ResourceAccessError::Access(AccessError::RevalidationRequired)
                )
            {
                AdapterError::InputAuthorityUnavailable(
                    "fresh input authority could not be verified".into(),
                )
            } else {
                refused()
            }
        })?;
        let agent = snapshot.lineage.agent.as_ref().ok_or_else(refused)?;
        let parent = &snapshot.lineage.sandbox;
        if agent.id != self.target.agent_id
            || agent.conversation.conversation_id.as_ref() != Some(&self.target.conversation_id)
            || agent.conversation.runtime_generation != Some(self.target.runtime_generation)
            || agent.conversation.active_turn != Some(self.turn_sequence)
            || agent.requested_stop_reason.is_some()
            || !agent.status.is_live()
            || parent.lifecycle.checkpoint_capture.is_some()
            || !matches!(
                parent.status,
                SandboxStatus::Active | SandboxStatus::CredentialRotating
            )
            || !agent
                .conversation
                .operation
                .as_ref()
                .is_some_and(|operation| {
                    operation.sequence == self.operation_sequence
                        && operation.turn_sequence == self.turn_sequence
                        && operation.runtime_generation == self.target.runtime_generation
                        && operation.state == AgentOperationDelivery::DispatchFenced
                })
        {
            return Err(refused());
        }
        self.authority
            .fence()
            .require_current()
            .map_err(|_| refused())
    }
}

pub(in crate::sandbox) struct TriggerDispatchGuard {
    pub authority: super::TriggerExecutionAuthorization,
    pub target: AgentRuntimeTarget,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
}

#[async_trait::async_trait]
impl ConversationInputGuard for TriggerDispatchGuard {
    async fn check(&self) -> layer3_adapter::Result<()> {
        let refused = || {
            AdapterError::Rejected("Trigger first-message authority is no longer current".into())
        };
        let proof = self
            .authority
            .current_proof()
            .await
            .map_err(|_| refused())?;
        let agent = proof.agent();
        if agent.id != self.target.agent_id
            || agent.conversation.conversation_id.as_ref() != Some(&self.target.conversation_id)
            || agent.conversation.runtime_generation != Some(self.target.runtime_generation)
            || agent.conversation.active_turn != Some(self.turn_sequence)
            || agent
                .conversation
                .first_message
                .as_ref()
                .is_none_or(|first| {
                    first.fence_sequence != self.operation_sequence
                        || first.turn_sequence != Some(self.turn_sequence)
                })
            || agent
                .conversation
                .operation
                .as_ref()
                .is_none_or(|operation| {
                    operation.sequence != self.operation_sequence
                        || operation.turn_sequence != self.turn_sequence
                        || operation.runtime_generation != self.target.runtime_generation
                        || operation.state != AgentOperationDelivery::DispatchFenced
                })
        {
            return Err(refused());
        }
        Ok(())
    }
}
