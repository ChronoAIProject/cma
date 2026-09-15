use async_trait::async_trait;

use crate::ids::AgentId;
use crate::sandbox::{Agent, AgentLogEntry, AgentLogKind, Sandbox, SandboxStatus};

use super::{SandboxMutationGuard, SandboxStoreError};

/// One generation-fenced, all-or-nothing conversation mutation.
#[derive(Debug, Clone)]
pub struct AgentConversationMutation {
    /// Ledger claim bound to the new runtime operation in this same transaction.
    pub command_claim: Option<crate::sandbox::agent_io::CommandClaim>,
    /// Opaque current resource permission; never attached to outcome evidence.
    pub execution_fence: Option<std::sync::Arc<crate::sandbox::agent_io::CommandExecutionFence>>,
    /// Exact Channel invoke authority, independently fenced with original source.
    pub channel_execution_fence:
        Option<std::sync::Arc<crate::sandbox::agent_io::ChannelExecutionFence>>,
    /// Exact parent authority for newly admitted input; absent for runtime evidence.
    pub admission_guard: Option<SandboxMutationGuard>,
    pub expected_agent_revision: u64,
    pub expected_last_seq: u64,
    pub runtime_generation: u64,
    pub agent: Agent,
    pub appended_at: String,
    pub agent_logs: Vec<AgentLogKind>,
}

/// Validate the parent read in the same transaction as the Agent mutation. A store
/// must also write-fence this parent when the guard is present.
#[doc(hidden)]
pub fn validate_agent_conversation_admission(
    current: &Agent,
    parent: Option<&Sandbox>,
    mutation: &AgentConversationMutation,
) -> Result<(), SandboxStoreError> {
    let admission = admits_input(current, mutation);
    if mutation.execution_fence.is_some() && mutation.channel_execution_fence.is_some() {
        return Err(SandboxStoreError::Conflict);
    }
    if (mutation.execution_fence.is_some() || mutation.channel_execution_fence.is_some())
        && (mutation.command_claim.is_none() || mutation.admission_guard.is_none() || !admission)
    {
        return Err(SandboxStoreError::Conflict);
    }
    let Some(guard) = mutation.admission_guard.as_ref() else {
        return if admission {
            Err(SandboxStoreError::Conflict)
        } else {
            Ok(())
        };
    };
    let parent = parent.ok_or(SandboxStoreError::Conflict)?;
    if !admission
        || parent.id != current.sandbox_id
        || parent.owner != current.owner
        || parent.id != mutation.agent.sandbox_id
        || parent.owner != mutation.agent.owner
    {
        return Err(SandboxStoreError::Conflict);
    }
    if parent.lifecycle.checkpoint_capture.is_some() {
        return Err(SandboxStoreError::CheckpointInProgress);
    }
    if *guard != SandboxMutationGuard::capture(parent)
        || !matches!(
            parent.status,
            SandboxStatus::Active
                | SandboxStatus::CredentialRotating
                | SandboxStatus::Pausing
                | SandboxStatus::Paused
                | SandboxStatus::Reviving
                | SandboxStatus::Rotating
        )
    {
        return Err(SandboxStoreError::Conflict);
    }
    parent
        .validate()
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

fn admits_input(current: &Agent, mutation: &AgentConversationMutation) -> bool {
    let next = &mutation.agent.conversation;
    let previous = &current.conversation;
    mutation.agent_logs.iter().any(|kind| {
        matches!(
            kind,
            AgentLogKind::OwnerMessage { .. } | AgentLogKind::TurnStarted { .. }
        )
    }) || next.operation.as_ref().is_some_and(|operation| {
        previous
            .operation
            .as_ref()
            .is_none_or(|old| old.sequence != operation.sequence)
    }) || next.queued_messages.iter().any(|message| {
        !previous
            .queued_messages
            .iter()
            .any(|old| old.sequence == message.sequence)
    }) || next.first_message.as_ref().is_some_and(|message| {
        previous.first_message.as_ref().is_none_or(|old| {
            old.attempt != message.attempt || old.recovery_generation != message.recovery_generation
        })
    })
}

/// Durable agent conversation state and its gap-free semantic log.
#[async_trait]
pub trait AgentConversationStore: Send + Sync {
    async fn commit_agent_conversation(
        &self,
        mutation: AgentConversationMutation,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;

    async fn pending_agent_approvals(
        &self,
        id: &AgentId,
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError>;
}
